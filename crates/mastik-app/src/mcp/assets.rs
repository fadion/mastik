//! `search_assets` and `place_asset` (§8.4, Phase 4): the assets browser's icon
//! sets, kit icon packs and photo services, for an agent.
//!
//! Only what the person switched on in Settings answers; an agent never switches
//! a set or service on, since that reaches the network. A call that waits on the
//! network says so ([`Step::Wait`]) and is run again as downloads land
//! (`Engine::run_later`). A placement takes the panel's credit, undo step name and
//! Unsplash download report.

use super::writes::commit_named;
use super::{Answer, Documents};
use crate::assets::panel::{self, Hit, KitPack};
use crate::assets::photos::{Orientation, PhotoError, PhotoSize, SERVICES, ServiceSpec};
use crate::assets::sets::{self, IconSet, SETS, SetSpec};
use crate::assets::{Assets, Poll, SetState};
use crate::kits::index::KitEntry;
use crate::theme::icon;
use mastik_core::kurbo::{Affine, Point, Size};
use mastik_core::{NodeId, NodeKind, Operation, Transaction};
use mastik_mcp::queue::Call;
use mastik_mcp::wire::Content;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 2] = ["search_assets", "place_asset"];

/// How long a call waits on the network before it is answered without what it
/// asked for: inside the queue's 30 seconds, so the agent hears before it
/// gives up.
pub(super) const WAIT: std::time::Duration = std::time::Duration::from_secs(25);

/// The most thumbnail bytes one search answers with; past it photos are listed
/// without one.
const THUMB_BUDGET: usize = 64 * 1024;

/// Why a host that lends no assets browser answers no asset tool.
pub(super) const NO_DESK: &str = "This host has no assets browser to search.";

/// A call's answer, or that it waits on a download.
pub(super) enum Step {
    Done(Answer),
    Wait,
}

/// What a host lends the asset tools: the window, or `serve`.
pub(crate) trait Desk {
    fn prefs(&self) -> &crate::prefs::AssetPrefs;
    fn assets(&mut self) -> &mut Assets;
    /// The photo services' keys as the session read them; `None` while unread.
    fn keys(&self) -> Option<std::collections::BTreeMap<String, String>>;
    /// The kit icon packs switched on, the document whose kit id is `own` left
    /// out, each read or not yet.
    fn packs(&mut self, own: Option<&str>) -> Vec<(KitEntry, Pack)>;
    /// Give a downloaded photo's pixels to the canvas's decode cache.
    fn adopt(&mut self, image: &mastik_core::ImageId, pixels: mastik_core::peniko::ImageData);
}

/// Where a kit icon pack's read stands.
pub(crate) enum Pack {
    Ready(Arc<KitPack>),
    Reading,
    Failed,
}

/// An asset id as a search answers it.
enum Asset {
    Icon {
        set: String,
        name: String,
        variant: String,
    },
    Kit {
        kit: String,
        main: String,
    },
    Photo {
        service: String,
        id: String,
    },
}

/// `icon:<set>/<name>/<variant>`, `kit:<kit>/<main>` or `photo:<service>/<id>`.
fn parse(asset: &str) -> Result<Asset, String> {
    let bad =
        || format!("`asset`: `{asset}` is no asset id; pass one `search_assets` answered with");
    let (kind, rest) = asset.split_once(':').ok_or_else(bad)?;
    match kind {
        "icon" => {
            let mut parts = rest.splitn(3, '/');
            match (parts.next(), parts.next(), parts.next()) {
                (Some(set), Some(name), Some(variant)) => Ok(Asset::Icon {
                    set: set.to_owned(),
                    name: name.to_owned(),
                    variant: variant.to_owned(),
                }),
                _ => Err(bad()),
            }
        }
        "kit" => {
            let (kit, main) = rest.rsplit_once('/').ok_or_else(bad)?;
            Ok(Asset::Kit {
                kit: kit.to_owned(),
                main: main.to_owned(),
            })
        }
        "photo" => {
            let (service, id) = rest.split_once('/').ok_or_else(bad)?;
            Ok(Asset::Photo {
                service: service.to_owned(),
                id: id.to_owned(),
            })
        }
        _ => Err(bad()),
    }
}

/// The queries a search asks, each trimmed.
fn queries(args: &Value) -> Result<Vec<String>, String> {
    let list: Vec<String> = match args.get("queries") {
        Some(Value::String(one)) => vec![one.clone()],
        Some(Value::Array(many)) => many
            .iter()
            .map(|q| q.as_str().map(|s| s.trim().to_owned()))
            .collect::<Option<_>>()
            .ok_or("`queries`: a list of words to search for")?,
        _ => return Err("`queries`: a list of words to search for, one search each".into()),
    };
    if list.is_empty() {
        return Err("`queries` is empty".into());
    }
    Ok(list)
}

/// `search_assets`: icons or photos answering each query. `own` is the current
/// document's kit id, whose own icons are not offered.
pub(super) fn search(
    docs: &mut impl Documents,
    args: &Value,
    own: Option<&str>,
) -> Result<Step, String> {
    let desk = docs.desk().ok_or(NO_DESK)?;
    let queries = queries(args)?;
    let source = args.get("source").and_then(Value::as_str);
    match args.get("kind").and_then(Value::as_str) {
        Some("photos") => photos(desk, &queries, source, args),
        _ => icons(desk, &queries, source, args, own),
    }
}

/// The most results a query answers with, by `limit` and the kind's default.
fn limit(args: &Value, default: usize) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map_or(default, |n| n as usize)
        .clamp(1, 30)
}

fn icons(
    desk: &mut dyn Desk,
    queries: &[String],
    source: Option<&str>,
    args: &Value,
    own: Option<&str>,
) -> Result<Step, String> {
    let on = desk.prefs().icon_sets.clone();
    let specs: Vec<&'static SetSpec> = SETS
        .iter()
        .filter(|s| on.iter().any(|k| k == s.key))
        .collect();
    let packs = desk.packs(own);
    if specs.is_empty() && packs.is_empty() {
        return Err("No icon set is switched on. The person switches sets on in Settings › Assets; an agent cannot.".into());
    }
    let wanted = |key: &str, name: &str| {
        source.is_none_or(|s| s.eq_ignore_ascii_case(key) || s.eq_ignore_ascii_case(name))
    };
    let specs: Vec<&'static SetSpec> = specs
        .into_iter()
        .filter(|s| wanted(s.key, s.name))
        .collect();
    let packs: Vec<(KitEntry, Pack)> = packs
        .into_iter()
        .filter(|(k, _)| wanted(&k.id, &k.name))
        .collect();
    if specs.is_empty() && packs.is_empty() {
        let names: Vec<String> = SETS
            .iter()
            .filter(|s| on.iter().any(|k| k == s.key))
            .map(|s| s.key.to_owned())
            .chain(desk.packs(own).into_iter().map(|(k, _)| k.name))
            .collect();
        return Err(format!(
            "`source`: no set switched on is called `{}`; these are: {}",
            source.unwrap_or_default(),
            names.join(", ")
        ));
    }
    // A variant is one set's: under several, each set shows its default.
    let variant = match args.get("variant").and_then(Value::as_str) {
        None => None,
        Some(label) => match (specs.as_slice(), packs.is_empty()) {
            ([one], true) => Some(
                one.variants
                    .iter()
                    .position(|v| v.label.eq_ignore_ascii_case(label))
                    .ok_or_else(|| {
                        let labels: Vec<&str> = one.variants.iter().map(|v| v.label).collect();
                        format!(
                            "`variant`: {} has no `{label}`; it has {}",
                            one.name,
                            labels.join(", ")
                        )
                    })?,
            ),
            _ => return Err("`variant` needs `source`, one icon set".into()),
        },
    };
    let mut ready: Vec<(&'static SetSpec, Arc<IconSet>)> = Vec::new();
    let mut kits: Vec<Arc<KitPack>> = Vec::new();
    let mut waiting = false;
    let mut unavailable = Vec::new();
    for spec in specs {
        match desk.assets().set(spec) {
            SetState::Ready(set) => ready.push((spec, Arc::clone(set))),
            SetState::Loading { .. } => waiting = true,
            SetState::Failed => unavailable.push(format!("{} could not be downloaded", spec.name)),
        }
    }
    for (entry, pack) in packs {
        match pack {
            Pack::Ready(p) => kits.push(p),
            Pack::Reading => waiting = true,
            Pack::Failed => unavailable.push(format!("{} could not be read", entry.name)),
        }
    }
    if waiting {
        return Ok(Step::Wait);
    }
    if ready.is_empty() && kits.is_empty() {
        return Err(format!("{}.", unavailable.join("; ")));
    }
    let limit = limit(args, 12);
    let results: Vec<Value> = queries
        .iter()
        .map(|q| {
            let hits = panel::find_icons(q, &ready, &kits, variant);
            json!({
                "query": q,
                "found": hits.len(),
                "assets": hits.iter().take(limit).map(icon_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    let mut answer = json!({ "results": results });
    if !unavailable.is_empty() {
        answer["unavailable"] = json!(unavailable);
    }
    Ok(Step::Done(Answer::Json(answer)))
}

/// One icon as a search answers it.
fn icon_json(hit: &Hit) -> Value {
    match hit {
        Hit::Set { spec, set, index } => {
            let icon = &set.icons[*index];
            let variant = spec.variants.get(icon.variant).map_or("", |v| v.label);
            let mut v = json!({
                "asset": format!("icon:{}/{}/{variant}", spec.key, icon.name),
                "name": icon.name,
                "set": spec.name,
                "variant": variant,
                "size": set.grid_of(icon.variant),
                "licence": spec.licence,
            });
            if !icon.aliases.is_empty() {
                v["aliases"] = json!(icon.aliases);
            }
            if spec.credit_required {
                v["credit_required"] = json!(true);
            }
            v
        }
        Hit::Kit { pack, top } => {
            let doc = pack.published.document();
            json!({
                "asset": format!("kit:{}/{}", pack.entry.id, top.to_wire()),
                "name": doc.get(*top).map_or("", |n| n.name()),
                "kit": pack.entry.name,
                "size": pack.grid,
            })
        }
    }
}

/// The service a photo search asks: `source` by key or name, else the panel's,
/// else the first switched on.
fn service_of(desk: &dyn Desk, source: Option<&str>) -> Result<&'static ServiceSpec, String> {
    let on: Vec<&'static ServiceSpec> = SERVICES
        .iter()
        .filter(|s| desk.prefs().photo_services.iter().any(|k| k == s.key))
        .collect();
    if on.is_empty() {
        return Err("No photo service is switched on. The person switches services on in Settings › Assets; an agent cannot.".into());
    }
    match source {
        Some(s) => on
            .iter()
            .copied()
            .find(|svc| svc.key.eq_ignore_ascii_case(s) || svc.name.eq_ignore_ascii_case(s))
            .ok_or_else(|| {
                let names: Vec<&str> = on.iter().map(|s| s.key).collect();
                format!(
                    "`source`: no service switched on is called `{s}`; these are: {}",
                    names.join(", ")
                )
            }),
        None => Ok(on
            .iter()
            .copied()
            .find(|s| s.key == desk.prefs().service)
            .unwrap_or(on[0])),
    }
}

/// A photo service's error, said as the panel's empty state says it.
fn photo_error(err: &PhotoError, service: &ServiceSpec) -> String {
    let (head, line) = match err {
        PhotoError::NoKey => (
            format!("{} needs a key", service.name),
            "The person adds it in Settings › Assets.",
        ),
        PhotoError::Refused => (
            format!("{} refused the key", service.name),
            "The person checks it in Settings › Assets.",
        ),
        PhotoError::Limited => (
            format!("{}'s hourly limit is reached", service.name),
            "Try again in an hour.",
        ),
        PhotoError::Unreachable => (
            format!("Can't reach {}", service.name),
            "Check the connection, or try again shortly.",
        ),
    };
    format!("{head}. {line}")
}

fn photos(
    desk: &mut dyn Desk,
    queries: &[String],
    source: Option<&str>,
    args: &Value,
) -> Result<Step, String> {
    let service = service_of(desk, source)?;
    if queries.iter().any(String::is_empty) {
        return Err("`queries`: a photo search needs words".into());
    }
    let orientation = match args.get("orientation").and_then(Value::as_str) {
        Some("landscape") => Orientation::Landscape,
        Some("portrait") => Orientation::Portrait,
        Some("square") => Orientation::Square,
        _ => Orientation::Any,
    };
    let keys = desk.keys();
    let mut pages = Vec::new();
    let mut waiting = false;
    for q in queries {
        match desk
            .assets()
            .agent_search((service.key, q.clone(), orientation), keys.as_ref())
        {
            Poll::Waiting => waiting = true,
            Poll::Ready(Ok(page)) => pages.push((q, page)),
            Poll::Ready(Err(e)) => {
                // A search asks one service: the others switched on are the way on.
                let others: Vec<String> = SERVICES
                    .iter()
                    .filter(|s| s.key != service.key)
                    .filter(|s| desk.prefs().photo_services.iter().any(|k| k == s.key))
                    .map(|s| format!("`{}`", s.key))
                    .collect();
                let mut said = photo_error(&e, service);
                if !others.is_empty() {
                    said.push_str(&format!(
                        " Or search another: `source` {}.",
                        others.join(", ")
                    ));
                }
                return Err(said);
            }
        }
    }
    if waiting {
        return Ok(Step::Wait);
    }
    let limit = limit(args, 6);
    let mut thumbs = Vec::new();
    for (_, page) in &pages {
        for photo in page.photos.iter().take(limit) {
            match desk.assets().agent_thumb(&photo.thumb) {
                Poll::Waiting => waiting = true,
                Poll::Ready(jpeg) => thumbs.push(jpeg),
            }
        }
    }
    if waiting {
        return Ok(Step::Wait);
    }
    let size = PhotoSize::from_key(&desk.prefs().photo_size);
    let today = panel::today();
    let mut contents = Vec::new();
    let mut spent = 0;
    let mut cut = false;
    let mut thumbs = thumbs.into_iter();
    let results: Vec<Value> = pages
        .iter()
        .map(|(q, page)| {
            let assets: Vec<Value> = page
                .photos
                .iter()
                .take(limit)
                .map(|p| {
                    let asset = format!("photo:{}/{}", p.service, p.id);
                    match thumbs.next().flatten() {
                        Some(jpeg) if spent + jpeg.len() <= THUMB_BUDGET => {
                            spent += jpeg.len();
                            contents.push(Content::Text(asset.clone()));
                            contents.push(Content::Image {
                                bytes: jpeg,
                                mime: "image/jpeg",
                            });
                        }
                        Some(_) => cut = true,
                        None => {}
                    }
                    let credit = p.credit(&today);
                    let mut v = json!({
                        "asset": asset,
                        "alt": p.title,
                        "credit": credit.text,
                        "licence": p.licence,
                    });
                    if let Some((w, h)) = p.size {
                        v["size"] = json!([w, h]);
                    }
                    v
                })
                .collect();
            json!({ "query": q, "found": page.total, "assets": assets })
        })
        .collect();
    let mut answer = json!({
        "service": service.name,
        "lands_at": size.label(),
        "results": results,
    });
    if cut {
        answer["said"] = json!(
            "Thumbnails are shown for the first photos only; ask with a smaller `limit` to see each."
        );
    }
    Ok(Step::Done(Answer::Files(answer, contents)))
}

/// Whether `place_asset` with `args` waits on a download: its icon set still
/// loading, or its photo not yet downloaded, which this asks for. Asked before
/// the call reads the document, so a call that waits reports nothing as seen.
/// A call that will be refused does not wait, nor download; [`place`] says why.
pub(super) fn waits(docs: &mut impl Documents, path: Option<&Path>, args: &Value) -> bool {
    let asset = args.get("asset").and_then(Value::as_str).map(parse);
    if let Some(Ok(Asset::Photo { .. })) = asset
        && path.is_none_or(|p| targets(docs, p, args).is_err())
    {
        return false;
    }
    let Some(desk) = docs.desk() else {
        return false;
    };
    match asset {
        Some(Ok(Asset::Icon { set, .. })) => SetSpec::by_key(&set)
            .filter(|s| desk.prefs().icon_sets.iter().any(|k| k == s.key))
            .is_some_and(|spec| matches!(desk.assets().set(spec), SetState::Loading { .. })),
        Some(Ok(Asset::Photo { service, id })) => {
            if !desk.prefs().photo_services.contains(&service) {
                return false;
            }
            let px = PhotoSize::from_key(&desk.prefs().photo_size);
            match desk.assets().agent_found(&service, &id) {
                Some(photo) => !desk.assets().agent_photo_ready(&photo, px),
                None => false,
            }
        }
        _ => false,
    }
}

/// Why a photo's placement is refused before its download: a `fill` layer that
/// takes no photo, as the panel fills only what can, or a missing `parent`.
fn targets(docs: &mut impl Documents, path: &Path, args: &Value) -> Result<(), String> {
    let s = docs.session(path).ok_or("the document closed")?;
    match super::nodes(args, "fill")? {
        Some(ids) => match ids.iter().find(|id| !panel::fillable(&s.doc, **id)) {
            Some(id) => Err(format!(
                "`{}` cannot take a photo: a photo fills an unlocked shape or frame, not text, a line, a spiral, an arc or a part of an icon",
                id.to_wire()
            )),
            None => Ok(()),
        },
        None => match super::node(args, "parent")? {
            None => Err("`parent`: the frame, group or page to place it in".into()),
            Some(p) if s.doc.get(p).is_none() => {
                Err(format!("`parent`: no layer `{}`", p.to_wire()))
            }
            Some(_) => Ok(()),
        },
    }
}

/// A number argument, refused when it is no positive number.
fn length(args: &Value, name: &str) -> Result<Option<f64>, String> {
    match args.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(v) => match v.as_f64() {
            Some(n) if n > 0.0 && n.is_finite() => Ok(Some(n)),
            _ => Err(format!("`{name}`: a length above 0")),
        },
    }
}

/// A placement's asset made ready: an icon's drawing or a photo's pixels, with
/// the credit it carries. What `place_asset` lays down at once, and what a
/// proposal holds until the person applies it, so applying reaches no network.
#[derive(Clone)]
pub(crate) enum Held {
    Icon {
        name: String,
        svg: String,
        credit: mastik_core::Credit,
    },
    Photo {
        image: Box<crate::app::LoadedImage>,
        credit: mastik_core::Credit,
        /// The service's photo, which is told it was used once it lands.
        photo: Box<crate::assets::photos::Photo>,
    },
}

impl PartialEq for Held {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Held::Icon { svg: a, .. }, Held::Icon { svg: b, .. }) => a == b,
            (Held::Photo { image: a, .. }, Held::Photo { image: b, .. }) => a.id == b.id,
            _ => false,
        }
    }
}

impl std::fmt::Debug for Held {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Held::Icon { name, .. } => write!(f, "Held::Icon({name})"),
            Held::Photo { photo, .. } => write!(f, "Held::Photo({})", photo.id),
        }
    }
}

/// [`hold`]'s answer: the asset, or that it waits on a download.
pub(crate) enum Holding {
    Ready(Box<Held>),
    Wait,
}

/// Where a placement lands, read from `place_asset`'s arguments.
struct Target {
    fill: Option<Vec<NodeId>>,
    parent: Option<NodeId>,
    at: Point,
    size: Option<f64>,
}

/// `args`' landing place: the layers a photo fills, else the parent and the
/// box's corner in it, and the size asked for.
fn target(args: &Value) -> Result<Target, String> {
    Ok(Target {
        fill: match super::nodes(args, "fill")? {
            Some(ids) if ids.is_empty() => return Err("`fill` is empty".into()),
            other => other,
        },
        parent: super::node(args, "parent")?,
        at: Point::new(
            args.get("x").and_then(Value::as_f64).unwrap_or(0.0),
            args.get("y").and_then(Value::as_f64).unwrap_or(0.0),
        ),
        size: length(args, "size")?,
    })
}

/// The colour `args` give an icon's ink, `#rrggbb` or `#rrggbbaa`, else black
/// as the panel draws it.
fn icon_ink(args: &Value) -> Result<&str, String> {
    let Some(v) = args.get("colour") else {
        return Ok(panel::ICON_INK);
    };
    v.as_str()
        .filter(|s| {
            s.strip_prefix('#').is_some_and(|h| {
                matches!(h.len(), 6 | 8) && h.bytes().all(|b| b.is_ascii_hexdigit())
            })
        })
        .ok_or_else(|| format!("`colour`: {v} is not a colour; write \"#rrggbb\""))
}

/// The refusal of an icon asked to fill layers.
const ICON_FILLS_NOTHING: &str =
    "An icon fills no layer; a photo does. Place the icon with `parent`.";
/// The refusal of a placement with nowhere to go.
const NO_PARENT: &str = "`parent`: the frame, group or page to place it in";

/// Ready the asset `place_asset`'s `args` name, for the document at `path`: a
/// switched-on set's icon drawn at its size, or a photo from a recent search
/// downloaded and given to the canvas. A kit's icon is no held asset; it is an
/// instance its kit tool places.
pub(crate) fn hold(
    docs: &mut impl Documents,
    path: &Path,
    args: &Value,
) -> Result<Holding, String> {
    let asset = args
        .get("asset")
        .and_then(Value::as_str)
        .ok_or("`asset`: an id `search_assets` answered with")?;
    let t = target(args)?;
    match parse(asset)? {
        Asset::Icon { set, name, variant } => {
            if t.fill.is_some() {
                return Err(ICON_FILLS_NOTHING.into());
            }
            t.parent.ok_or(NO_PARENT)?;
            let ink = icon_ink(args)?;
            icon_drawing(docs, (&set, &name, &variant), t.size, ink)
        }
        Asset::Kit { .. } => {
            Err("A kit's icon is an instance of its component, placed at Edit access".into())
        }
        Asset::Photo { service, id } => {
            if args.get("colour").is_some() {
                return Err("`colour` inks an icon; a photo has no ink".into());
            }
            let desk = docs.desk().ok_or(NO_DESK)?;
            if !desk.prefs().photo_services.contains(&service) {
                return Err(format!(
                    "`{service}` is not switched on in Settings › Assets; an agent cannot switch it on"
                ));
            }
            let photo = desk.assets().agent_found(&service, &id).ok_or(
                "That photo is in no recent search: search again with `search_assets`, and place one it answers",
            )?;
            targets(docs, path, args)?;
            let desk = docs.desk().ok_or(NO_DESK)?;
            let px = PhotoSize::from_key(&desk.prefs().photo_size);
            let (image, pixels) = match desk.assets().agent_photo(&photo, px) {
                Poll::Waiting => return Ok(Holding::Wait),
                Poll::Ready(read) => read?,
            };
            desk.adopt(&image.id, pixels);
            let credit = photo.credit(&panel::today());
            Ok(Holding::Ready(Box::new(Held::Photo {
                image: Box::new(image),
                credit,
                photo: Box::new(photo),
            })))
        }
    }
}

/// Lay `held` down on `s` where `args` say, as the panel does: an icon in its
/// parent, a photo in its parent or filling layers, each one undo step under
/// the panel's name.
pub(crate) fn put_held(
    s: &mut crate::session::EditorSession,
    client: &str,
    args: &Value,
    held: &Held,
) -> Result<Value, String> {
    let t = target(args)?;
    match held {
        Held::Icon { name, svg, credit } => {
            if t.fill.is_some() {
                return Err(ICON_FILLS_NOTHING.into());
            }
            let parent = t.parent.ok_or(NO_PARENT)?;
            put_icon(s, client, parent, t.at, name, svg, credit)
        }
        Held::Photo { image, credit, .. } => match t.fill {
            Some(ids) => fill_photo(s, client, &ids, image, credit),
            None => {
                let parent = t.parent.ok_or(NO_PARENT)?;
                put_photo(s, client, parent, t.at, t.size, image, credit)
            }
        },
    }
}

/// Tell `held`'s photo service its photo was used, as the panel does once one
/// lands; nothing for an icon.
pub(crate) fn report_used(desk: &mut dyn Desk, held: &Held) {
    if let Held::Photo { photo, .. } = held {
        let keys = desk.keys();
        desk.assets().report_download(photo, keys.as_ref());
    }
}

/// `place_asset`: an icon or photo at a place in a parent, or a photo filling
/// layers, on the document at `path`.
pub(super) fn place(docs: &mut impl Documents, path: &Path, call: &Call) -> Result<Step, String> {
    let args = &call.args;
    let asset = args
        .get("asset")
        .and_then(Value::as_str)
        .ok_or("`asset`: an id `search_assets` answered with")?;
    match parse(asset)? {
        Asset::Kit { kit, main } => {
            let t = target(args)?;
            if t.fill.is_some() {
                return Err(ICON_FILLS_NOTHING.into());
            }
            if args.get("colour").is_some() {
                return Err(
                    "`colour` inks an icon from a set; a kit's icon is an instance, recoloured with `set` once placed".into(),
                );
            }
            let parent = t.parent.ok_or(NO_PARENT)?;
            let at = t.at;
            let desk = docs.desk().ok_or(NO_DESK)?;
            if !desk.prefs().icon_sets.contains(&sets::pack_key(&kit)) {
                return Err(format!(
                    "The kit `{kit}` is not switched on in Settings › Assets; an agent cannot switch it on"
                ));
            }
            // A kit's icon is an instance of its main, as the panel places it.
            let mut inner = call.again();
            inner.tool = "insert_from_kit".into();
            inner.args = json!({
                "kit": kit,
                "main": main,
                "parent": parent.to_wire(),
                "x": at.x,
                "y": at.y,
            });
            super::kits::run(docs, path, &inner).map(Step::Done)
        }
        Asset::Icon { .. } | Asset::Photo { .. } => {
            let held = match hold(docs, path, args)? {
                Holding::Wait => return Ok(Step::Wait),
                Holding::Ready(held) => *held,
            };
            let s = docs.session(path).ok_or("the document closed")?;
            let answer = put_held(s, &call.client, args, &held)?;
            report_used(docs.desk().ok_or(NO_DESK)?, &held);
            Ok(Step::Done(Answer::Json(answer)))
        }
    }
}

/// A remote set's icon drawn as the panel inserts it, `size` points across
/// (the variant's grid unless given) in `ink`, with its credit.
fn icon_drawing(
    docs: &mut impl Documents,
    (set, name, variant): (&str, &str, &str),
    size: Option<f64>,
    ink: &str,
) -> Result<Holding, String> {
    let desk = docs.desk().ok_or(NO_DESK)?;
    let spec = SetSpec::by_key(set).ok_or_else(|| format!("`asset`: no icon set `{set}`"))?;
    if !desk.prefs().icon_sets.iter().any(|k| k == spec.key) {
        return Err(format!(
            "{} is not switched on in Settings › Assets; an agent cannot switch it on",
            spec.name
        ));
    }
    let loaded = match desk.assets().set(spec) {
        SetState::Ready(s) => Arc::clone(s),
        SetState::Loading { .. } => return Ok(Holding::Wait),
        SetState::Failed => return Err(format!("{} could not be downloaded", spec.name)),
    };
    let v = spec
        .variants
        .iter()
        .position(|x| x.label.eq_ignore_ascii_case(variant))
        .ok_or_else(|| format!("`asset`: {} has no variant `{variant}`", spec.name))?;
    let icon = loaded
        .icons
        .iter()
        .find(|i| i.name == name && i.variant == v)
        .ok_or_else(|| format!("`asset`: {} has no icon `{name}`", spec.name))?;
    let size = size.unwrap_or_else(|| loaded.grid_of(v));
    Ok(Holding::Ready(Box::new(Held::Icon {
        name: icon.name.clone(),
        svg: icon.svg(size, ink),
        credit: panel::icon_credit(spec, &panel::today()),
    })))
}

/// A held icon as the panel inserts it: the SVG importer's group named after
/// the icon, its box's corner at `at` in `parent`, with its credit, as
/// *Insert icon*.
fn put_icon(
    s: &mut crate::session::EditorSession,
    client: &str,
    parent: NodeId,
    at: Point,
    name: &str,
    svg: &str,
    credit: &mastik_core::Credit,
) -> Result<Value, String> {
    let index = s
        .doc
        .get(parent)
        .ok_or_else(|| format!("`parent`: no layer `{}`", parent.to_wire()))?
        .children()
        .len();
    let mut out = mastik_core::svg_in::import(svg, &mut s.ids, parent, index, None)
        .map_err(|e| format!("{name} could not be read: {e}"))?;
    let root = out.root;
    if let Some(Operation::CreateNode {
        transform,
        name: layer,
        ..
    }) = out
        .tx
        .0
        .iter_mut()
        .find(|op| matches!(op, Operation::CreateNode { id, .. } if *id == root))
    {
        *transform = Some(Affine::translate(at.to_vec2()) * transform.unwrap_or(Affine::IDENTITY));
        *layer = Some(name.to_owned());
    }
    out.tx.0.push(Operation::SetCredit {
        id: root,
        credit: Some(Box::new(credit.clone())),
    });
    s.name_mcp_step_as(client, "place_asset", "Insert icon", icon::ASSETS);
    commit_named(s, out.tx)?;
    let mut answer = json!({ "created": [root.to_wire()] });
    if credit.required {
        answer["credit"] = json!(credit.text);
    }
    Ok(answer)
}

/// A photo as a new layer, its box's corner at `at` in `parent`: `width` across
/// at its own aspect, else its own size fitted into a frame parent, as a drop
/// fits it; *Insert photo*.
fn put_photo(
    s: &mut crate::session::EditorSession,
    client: &str,
    parent: NodeId,
    at: Point,
    width: Option<f64>,
    image: &crate::app::LoadedImage,
    credit: &mastik_core::Credit,
) -> Result<Value, String> {
    let intrinsic = image.intrinsic;
    let box_size = match width {
        Some(w) => Size::new(w, w * intrinsic.height / intrinsic.width.max(1.0)),
        None => match s.resolved.used_kind(&s.doc, parent) {
            Some(NodeKind::Artboard { size, .. }) => {
                mastik_core::edit::fitted_image_size(intrinsic, *size)
            }
            _ => intrinsic,
        },
    };
    let index = s
        .doc
        .get(parent)
        .ok_or_else(|| format!("`parent`: no layer `{}`", parent.to_wire()))?
        .children()
        .len();
    let node = s.ids.mint();
    let name = crate::canvas::image_layer_name(&s.doc, parent, image.name.clone());
    let known = s.doc.has_image(&image.id);
    let far = Point::new(at.x + box_size.width, at.y + box_size.height);
    let mut tx = mastik_core::edit::create_image_in_box(
        node,
        parent,
        index,
        Affine::IDENTITY,
        at,
        far,
        image.id.clone(),
        (!known).then(|| image.entry.clone()),
        Some(name),
    );
    tx.0.push(Operation::SetCredit {
        id: node,
        credit: Some(Box::new(credit.clone())),
    });
    s.name_mcp_step_as(client, "place_asset", "Insert photo", icon::IMAGE);
    commit_named(s, tx)?;
    Ok(json!({ "created": [node.to_wire()], "credit": credit.text }))
}

/// A photo as the fill of each of `ids`, each with its credit, as one
/// *Fill with photo*: the picture fill or top fill replaced, else a new fill.
fn fill_photo(
    s: &mut crate::session::EditorSession,
    client: &str,
    ids: &[NodeId],
    image: &crate::app::LoadedImage,
    credit: &mastik_core::Credit,
) -> Result<Value, String> {
    let mut advanced = s.doc.clone();
    let mut ops = Vec::new();
    for id in ids {
        if !panel::fillable(&advanced, *id) {
            return Err(format!("`{}` can no longer take a photo", id.to_wire()));
        }
        let mut tx = panel::fill_tx(&advanced, *id, image)
            .ok_or_else(|| format!("`{}` cannot take a photo", id.to_wire()))?;
        tx.0.push(Operation::SetCredit {
            id: *id,
            credit: Some(Box::new(credit.clone())),
        });
        advanced.apply(&tx).map_err(|e| format!("Refused: {e}"))?;
        ops.extend(tx.0);
    }
    s.name_mcp_step_as(client, "place_asset", "Fill with photo", icon::PAINT_BUCKET);
    commit_named(s, Transaction(ops))?;
    Ok(json!({
        "filled": ids.iter().map(|id| id.to_wire()).collect::<Vec<_>>(),
        "credit": credit.text,
    }))
}

#[cfg(test)]
mod tests {
    use super::super::Engine;
    use crate::app::MastikApp;
    use crate::assets::SetState;
    use crate::assets::photos::{Orientation, Page, Photo};
    use crate::assets::sets::{self, IconSet, SetSpec};
    use crate::mcp::Documents;
    use mastik_core::{NodeId, NodeKind, Operation, Transaction};
    use mastik_mcp::queue::Call;
    use mastik_mcp::wire::{Content, Output};
    use serde_json::{Value, json};
    use std::path::PathBuf;
    use std::sync::Arc;
    use std::sync::mpsc::{Receiver, sync_channel};

    /// A call answered now, refused if it fails or waits.
    fn ok(engine: &mut Engine, app: &mut MastikApp, tool: &str, args: Value) -> Value {
        let out = now(engine, app, tool, args);
        assert!(!out.is_error, "{tool}: {:?}", out.content);
        out.structured.unwrap_or(Value::Null)
    }

    /// A call's answer, which must not wait.
    fn now(engine: &mut Engine, app: &mut MastikApp, tool: &str, args: Value) -> Output {
        let (reply, _answer) = sync_channel(1);
        engine
            .handle_or_wait(app, Call::new(1, None, tool, args, reply))
            .unwrap_or_else(|| panic!("{tool} waited"))
    }

    /// A call that must wait; the channel its answer comes on.
    fn waits(
        engine: &mut Engine,
        app: &mut MastikApp,
        tool: &str,
        args: Value,
    ) -> Receiver<Output> {
        let (reply, answer) = sync_channel(1);
        let out = engine.handle_or_wait(app, Call::new(1, None, tool, args, reply));
        assert!(out.is_none(), "{tool} answered at once: {out:?}");
        answer
    }

    /// The app using a document in a file, holding a rectangle and a text layer:
    /// the engine, the file, its page, the rectangle and the text.
    fn fixture(name: &str) -> (MastikApp, Engine, PathBuf, String, NodeId, NodeId) {
        let dir =
            std::env::temp_dir().join(format!("mastik-mcp-assets-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("page.mastik");
        let mut ids = mastik_core::IdSource::new(crate::session::random_actor());
        let mut doc = mastik_core::Document::new(ids.mint());
        let page = doc.first_page();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: page,
            index: 0,
            kind: NodeKind::Rect {
                size: mastik_core::kurbo::Size::new(40.0, 30.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Hero image".into()),
        }]))
        .unwrap();
        std::fs::write(&file, mastik_core::io::save(&doc).unwrap()).unwrap();
        let ctx = egui::Context::default();
        let mut app = MastikApp::headless(&ctx);
        let mut engine = Engine::default();
        let document = json!({ "document": file.display().to_string() });
        ok(&mut engine, &mut app, "use_document", document);
        let page = page.to_wire();
        let made = ok(
            &mut engine,
            &mut app,
            "create_text",
            json!({ "parent": page, "text": "Hi" }),
        );
        let text = NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        (app, engine, file, page, rect, text)
    }

    /// The last undo step's name on the document at `file`.
    fn last_step(app: &mut MastikApp, file: &std::path::Path) -> String {
        let s = Documents::session(app, file).unwrap();
        s.history.done().last().expect("a step").note.label.clone()
    }

    /// Font Awesome switched on and loaded, holding one icon, `bell`.
    fn bell_set(app: &mut MastikApp) -> &'static SetSpec {
        let spec = SetSpec::by_key("font-awesome").unwrap();
        let set = Arc::new(IconSet {
            icons: vec![sets::Icon {
                name: "bell".into(),
                variant: 0,
                body: r#"<path fill="currentColor" d="M0 0h32v32H0z"/>"#.into(),
                view: [0.0, 0.0, 32.0, 32.0],
                aliases: vec!["alarm".into()],
            }],
            grid: 32.0,
            variant_grids: Vec::new(),
        });
        app.assets.sets.insert(spec.key, SetState::Ready(set));
        app.prefs.assets.icon_sets = vec![spec.key.into()];
        spec
    }

    /// An Unsplash photo, switched on, whose download Unsplash asks to hear of.
    fn coffee(app: &mut MastikApp) -> Photo {
        app.prefs.assets.photo_services = vec!["unsplash".into()];
        Photo {
            service: "unsplash",
            id: "x1".into(),
            size: Some((8, 6)),
            thumb: "https://i/thumb".into(),
            urls: [
                "https://i/small".into(),
                "https://i/regular".into(),
                "https://i/full".into(),
            ],
            title: "a coffee cup".into(),
            author: "Ana Ruiz".into(),
            author_url: "https://unsplash.com/@ana".into(),
            page_url: "https://unsplash.com/photos/x1".into(),
            licence: "Unsplash License".into(),
            licence_url: "https://unsplash.com/license".into(),
            report_url: Some("https://api.unsplash.com/photos/x1/download".into()),
        }
    }

    /// `photo`'s search and thumbnail, landed as the workers would.
    fn searched(app: &mut MastikApp, photo: &Photo) {
        let page = Page {
            photos: vec![photo.clone()],
            total: 1,
            more: false,
        };
        let key = ("unsplash", "coffee".to_owned(), Orientation::Any);
        app.assets.agent_land_search(key, Ok(page));
        app.assets
            .agent_land_thumb(&photo.thumb, Some(vec![0xff, 0xd8, 0xff]));
    }

    /// `photo` downloaded at the default size, as the worker would land it.
    fn downloaded(app: &mut MastikApp, photo: &Photo) {
        let png = crate::app::rgba_to_png(&[200u8; 8 * 6 * 4], (8, 6)).unwrap();
        let read = crate::app::read_picture(png, None, Some("a coffee cup".into()))
            .map_err(|_| String::new());
        app.assets.agent_land_photo(&photo.urls[1], read);
    }

    /// Nothing switched on answers that the person switches it on, never the
    /// agent; a search reads only the sets switched on.
    ///
    /// flip: `icons` reading every set rather than the ones on → fails at
    /// "nothing on".
    #[test]
    fn only_what_the_person_switched_on_is_searched() {
        let (mut app, mut engine, ..) = fixture("off");
        let args = json!({ "queries": ["bell"], "kind": "icons" });
        let out = now(&mut engine, &mut app, "search_assets", args.clone());
        let why = format!("{:?}", out.content);
        assert!(
            out.is_error && why.contains("Settings"),
            "nothing on: {why}"
        );
        let out = now(
            &mut engine,
            &mut app,
            "search_assets",
            json!({ "queries": ["coffee"], "kind": "photos" }),
        );
        assert!(out.is_error, "no service on");

        bell_set(&mut app);
        let found = ok(&mut engine, &mut app, "search_assets", args);
        let first = &found["results"][0]["assets"][0];
        assert_eq!(first["asset"], json!("icon:font-awesome/bell/Solid"));
        assert_eq!(first["aliases"], json!(["alarm"]));
        let alias = ok(
            &mut engine,
            &mut app,
            "search_assets",
            json!({ "queries": ["alarm", "nothing-like-it"], "kind": "icons" }),
        );
        assert_eq!(alias["results"][0]["found"], json!(1), "found by alias");
        assert_eq!(alias["results"][1]["found"], json!(0));
    }

    /// A photo search a service refuses names the other services switched on,
    /// since a search asks one.
    ///
    /// flip: `photos` answering the service's error alone → fails at "the other
    /// service".
    #[test]
    fn a_refused_photo_search_names_the_other_services() {
        let (mut app, mut engine, ..) = fixture("limited");
        app.prefs.assets.photo_services = vec!["unsplash".into(), "pexels".into()];
        app.prefs.assets.service = "unsplash".into();
        let key = ("unsplash", "coffee".to_owned(), Orientation::Any);
        app.assets
            .agent_land_search(key, Err(crate::assets::photos::PhotoError::Limited));
        let out = now(
            &mut engine,
            &mut app,
            "search_assets",
            json!({ "queries": ["coffee"], "kind": "photos" }),
        );
        let why = format!("{:?}", out.content);
        assert!(out.is_error && why.contains("hourly limit"), "{why}");
        assert!(why.contains("`pexels`"), "the other service: {why}");
    }

    /// An icon lands as the panel's insert does: a group named after it, its
    /// box's corner where asked, credited, as one *Insert icon* step.
    ///
    /// flip: `place_icon` without the `SetCredit` → fails at "credited".
    #[test]
    fn an_icon_lands_credited_where_asked() {
        let (mut app, mut engine, file, page, ..) = fixture("icon");
        bell_set(&mut app);
        let made = ok(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "icon:font-awesome/bell/Solid", "parent": page, "x": 10, "y": 20, "size": 24 }),
        );
        let root = NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        assert_eq!(last_step(&mut app, &file), "Insert icon");
        let s = Documents::session(&mut app, &file).unwrap();
        let node = s.doc.get(root).unwrap();
        assert_eq!(node.name(), "bell");
        let credit = node.credit().expect("credited");
        assert_eq!(credit.provider, "Font Awesome Free");
        let b = s.resolved.world_bounds(root).unwrap();
        assert_eq!(
            (b.x0.round(), b.y0.round(), b.width().round()),
            (10.0, 20.0, 24.0),
            "{b:?}"
        );
        let why = now(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "icon:font-awesome/bell/Solid", "fill": [page] }),
        );
        assert!(why.is_error, "an icon fills nothing");
    }

    /// An icon placed with `colour` is drawn in that ink, and a `colour` that is
    /// no hex colour is refused.
    ///
    /// flip: `hold` ignoring `icon_ink` → fails at "inked red".
    #[test]
    fn an_icon_takes_the_colour_asked() {
        let (mut app, mut engine, file, page, ..) = fixture("icon-ink");
        bell_set(&mut app);
        let made = ok(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "icon:font-awesome/bell/Solid", "parent": page, "colour": "#ff0000" }),
        );
        let root = NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        let s = Documents::session(&mut app, &file).unwrap();
        let mut inks = Vec::new();
        let mut stack = vec![root];
        while let Some(id) = stack.pop() {
            let n = s.doc.get(id).unwrap();
            stack.extend(n.children());
            for f in &n.paint().fills {
                if let mastik_core::Brush::Solid(c) = f.value.brush.drawn() {
                    inks.push(c.to_rgba8().to_u8_array());
                }
            }
        }
        assert!(!inks.is_empty(), "the icon paints");
        assert!(
            inks.iter().all(|c| c[..3] == [255, 0, 0]),
            "inked red: {inks:?}"
        );
        let why = now(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "icon:font-awesome/bell/Solid", "parent": page, "colour": "red" }),
        );
        assert!(why.is_error, "a colour name is refused");
    }

    /// A photo search waits for its answer and thumbnails, then answers each
    /// photo with its credit and thumbnail; a placement waits for the download,
    /// then lands credited as *Insert photo* and reports the download.
    ///
    /// flip: `run_later` not sending a finished call's answer → fails at
    /// "the search answered".
    #[test]
    fn a_photo_is_searched_downloaded_and_placed_after_waiting() {
        let (mut app, mut engine, file, page, ..) = fixture("photo");
        let photo = coffee(&mut app);
        let search = json!({ "queries": ["coffee"], "kind": "photos" });
        let answer = waits(&mut engine, &mut app, "search_assets", search);
        engine.run_later(&mut app);
        assert!(answer.try_recv().is_err(), "still waiting");
        searched(&mut app, &photo);
        engine.run_later(&mut app);
        let out = answer.try_recv().expect("the search answered");
        let found = out.structured.unwrap();
        let first = &found["results"][0]["assets"][0];
        assert_eq!(first["asset"], json!("photo:unsplash/x1"));
        assert_eq!(first["credit"], json!("Photo by Ana Ruiz on Unsplash"));
        assert!(
            out.content.iter().any(|c| matches!(
                c,
                Content::Image {
                    mime: "image/jpeg",
                    ..
                }
            )),
            "a thumbnail"
        );

        let place = json!({ "asset": "photo:unsplash/x1", "parent": page, "x": 5, "y": 5 });
        let answer = waits(&mut engine, &mut app, "place_asset", place);
        downloaded(&mut app, &photo);
        engine.run_later(&mut app);
        let made = answer.try_recv().expect("placed").structured.unwrap();
        let id = NodeId::from_wire(made["created"][0].as_str().unwrap()).unwrap();
        assert_eq!(last_step(&mut app, &file), "Insert photo");
        let s = Documents::session(&mut app, &file).unwrap();
        assert_eq!(
            s.doc.get(id).and_then(|n| n.credit()).unwrap().provider,
            "Unsplash"
        );
        assert_eq!(
            app.assets.reported,
            vec!["https://api.unsplash.com/photos/x1/download".to_string()]
        );
    }

    /// A photo fills the layers asked as one *Fill with photo* step, each
    /// credited; a layer that takes no photo is refused before any download.
    ///
    /// flip: `place` skipping the `fillable` check → fails at "text refused".
    #[test]
    fn a_photo_fills_layers_and_refuses_text() {
        let (mut app, mut engine, file, _page, rect, text) = fixture("fill");
        let photo = coffee(&mut app);
        searched(&mut app, &photo);
        let why = now(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "photo:unsplash/x1", "fill": [text.to_wire()] }),
        );
        assert!(why.is_error, "text refused");
        downloaded(&mut app, &photo);
        let filled = ok(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "photo:unsplash/x1", "fill": [rect.to_wire()] }),
        );
        assert_eq!(filled["filled"], json!([rect.to_wire()]));
        assert_eq!(last_step(&mut app, &file), "Fill with photo");
        let s = Documents::session(&mut app, &file).unwrap();
        assert!(
            s.doc.get(rect).and_then(|n| n.credit()).is_some(),
            "credited"
        );
    }

    /// A placement still downloading past the wait answers a pending id, keeps
    /// running, and `pending` answers what it made once it landed, once.
    ///
    /// flip: `run_later` dropping a call answered pending → fails at "made".
    #[test]
    fn a_slow_download_answers_pending_and_lands_later() {
        let (mut app, mut engine, _file, page, ..) = fixture("pending");
        let photo = coffee(&mut app);
        searched(&mut app, &photo);
        let place = json!({ "asset": "photo:unsplash/x1", "parent": page });
        let answer = waits(&mut engine, &mut app, "place_asset", place);
        engine.later[0].since -= super::WAIT;
        engine.run_later(&mut app);
        let pending = answer
            .try_recv()
            .expect("answered pending")
            .structured
            .unwrap();
        assert_eq!(pending["pending"], json!("p1"));
        let still = ok(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "pending": "p1" }),
        );
        assert_eq!(still["pending"], json!("p1"), "still downloading");

        downloaded(&mut app, &photo);
        engine.run_later(&mut app);
        let made = ok(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "pending": "p1" }),
        );
        assert!(made["created"][0].is_string(), "made: {made}");
        let again = now(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "pending": "p1" }),
        );
        assert!(again.is_error, "answered once");
    }

    /// What the person changed while a placement waited is reported in its answer:
    /// the wait is asked before the client is caught up.
    ///
    /// flip: `assets::waits` asked after `catch_up` in `Engine::handle_now` →
    /// fails at "changed".
    #[test]
    fn a_waiting_placement_still_reports_the_persons_edit() {
        let (mut app, mut engine, file, page, rect, _) = fixture("changed");
        let photo = coffee(&mut app);
        searched(&mut app, &photo);
        let s = Documents::session(&mut app, &file).unwrap();
        assert!(s.commit(Transaction(vec![Operation::SetName {
            id: rect,
            name: "Renamed".into(),
        }])));
        let answer = waits(
            &mut engine,
            &mut app,
            "place_asset",
            json!({ "asset": "photo:unsplash/x1", "parent": page }),
        );
        downloaded(&mut app, &photo);
        engine.run_later(&mut app);
        let made = answer.try_recv().expect("placed").structured.unwrap();
        assert!(made.get("changed").is_some(), "changed: {made}");
    }

    /// A proposal that places a photo waits on its download, changes nothing,
    /// and lands credited as one step named by its title when the person
    /// applies it, which is when Unsplash hears of it; read back without its
    /// photo, it refuses.
    ///
    /// flip: `propose::run` writing a `place_asset` call through `write` → fails
    /// at "proposed"; `assistant_apply` without `report_used` → fails at "reported
    /// on apply".
    #[test]
    fn a_proposal_places_its_photo_when_applied() {
        let (mut app, mut engine, file, _page, rect, _) = fixture("propose");
        let photo = coffee(&mut app);
        searched(&mut app, &photo);
        let ask = json!({
            "title": "Hero photo",
            "calls": [{ "verb": "place_asset",
                        "args": { "asset": "photo:unsplash/x1", "fill": [rect.to_wire()] } }],
        });
        let answer = waits(&mut engine, &mut app, "propose_edit", ask);
        let depth = Documents::session(&mut app, &file)
            .unwrap()
            .history
            .undo_depth();
        downloaded(&mut app, &photo);
        engine.run_later(&mut app);
        let out = answer.try_recv().expect("answered once downloaded");
        assert!(!out.is_error, "proposed: {:?}", out.content);
        let made = out.structured.unwrap();
        assert!(
            made["changes"][0]
                .as_str()
                .unwrap()
                .starts_with("Fill with photo"),
            "its line: {made}"
        );
        let s = Documents::session(&mut app, &file).unwrap();
        assert!(
            s.doc.get(rect).and_then(|n| n.credit()).is_none() && s.history.undo_depth() == depth,
            "nothing changed yet"
        );
        assert!(app.assets.reported.is_empty(), "not reported when proposed");

        let id = made["proposal"].as_u64().unwrap();
        let p = engine.proposal(id).expect("held").clone();
        app.assistant.proposals.push(p.clone());
        // A card applies to the document in front.
        let tab = app.tab_of_path(&file).expect("its tab");
        app.activate_tab(tab);
        assert!(
            app.session.doc.contains(rect),
            "the fixture: its tab in front"
        );
        app.assistant_apply(id);
        let s = Documents::session(&mut app, &file).unwrap();
        assert!(
            s.doc.get(rect).and_then(|n| n.credit()).is_some(),
            "credited"
        );
        assert_eq!(s.history.undo_depth(), depth + 1, "one step");
        assert_eq!(last_step(&mut app, &file), "Hero photo");
        assert_eq!(app.assets.reported.len(), 1, "reported on apply");

        // As read back from a saved thread.
        Documents::session(&mut app, &file).unwrap().undo();
        let bare = crate::mcp::propose::Proposal {
            held: Vec::new(),
            ..p
        };
        let s = Documents::session(&mut app, &file).unwrap();
        let why = crate::mcp::propose::apply(s, &bare).expect_err("no photo at hand");
        assert!(why.contains("no longer at hand"), "{why}");
    }

    /// A chat searches at any level and runs a placement only at Edit; Propose
    /// proposes one.
    ///
    /// flip: `access::allowed` without the `place_asset` arm → fails at "Edit places".
    #[test]
    fn a_chat_searches_at_look_and_places_at_edit() {
        use crate::mcp::access::{Level, allowed};
        assert!(allowed(Level::Look, "search_assets"));
        assert!(!allowed(Level::Look, "place_asset"));
        assert!(!allowed(Level::Propose, "place_asset"));
        assert!(allowed(Level::Edit, "place_asset"), "Edit places");
        assert!(
            !allowed(Level::Edit, "insert_from_kit"),
            "the rest of Phase 4 stays out"
        );
    }
}
