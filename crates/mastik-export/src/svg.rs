//! Semantic SVG export (§7.1).
//!
//! A pure function of `Document` + `Resolved`. Groups and artboards become nested
//! `<g>` elements carrying their local transform and opacity (so group opacity
//! composites as a unit), with shapes in local coordinates. Artboards clip with a
//! `clipPath` unless the layer's `clip` flag is off.
//!
//! Doubles as a model-completeness gate: anything the model can express and this
//! cannot write is a gap worth failing on.
//!
//! Text is one `<text>` per line with one `<tspan>` per run, so a viewer re-shapes
//! with its own fonts; line breaks and baselines are ours, from
//! `text::export_lines`. Asked for outlines ([`svg_with`]), text is instead the
//! glyph outlines the canvas draws, which no viewer can re-shape.

use mastik_core::geometry::Dots;
use mastik_core::kurbo::{Affine, BezPath, Insets, Rect, Shape, Vec2};
use mastik_core::peniko::{Color, Extend, GradientKind};
use mastik_core::{
    BlendMode, Decoration, Document, Effect, EffectKind, ExportSpec, ExportText, Fill, Framing,
    LineStyle, Markers, MaskMode, Node, NodeId, NodeKind, Paint, Resolved, Shadow, Stroke,
    StrokeAlign, SvgColour, SvgStrokes, SvgView, geometry,
};
use mastik_core::{Brush, Keyed};
use std::fmt::Write;

/// An export and what it could not say exactly — the writer's side of
/// `svg_in::Import`'s `skipped`/`approximated`.
///
/// [`Self::approximated`] is where the picture differs from the canvas;
/// [`Self::round_trip`] is where the picture is right but reading the file back
/// gives a different document (a flipped rail, an image `<pattern>`). Filing those
/// as approximated would tell a user their drawing is wrong.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Export {
    /// The markup.
    pub svg: String,
    /// Features a viewer draws slightly differently. Deduplicated, in order met.
    pub approximated: Vec<String>,
    /// Features drawn exactly whose document does not survive our own reader.
    pub round_trip: Vec<String>,
    /// How many different solid colours, opacity aside, `currentColor` stood in
    /// for — more than one is a choice the export panel names. Zero when the file
    /// keeps its colours.
    pub merged: usize,
}

/// The sink [`emit_node`] pushes into. Not built in `collect_defs`, which has no
/// visibility guard and would report features on layers the file lacks.
#[derive(Default)]
struct Report {
    approximated: Vec<String>,
    round_trip: Vec<String>,
}

impl Report {
    /// The picture differs. Deduplicated by wording, like the reader's.
    fn approximate(&mut self, what: &str) {
        if !self.approximated.iter().any(|s| s == what) {
            self.approximated.push(what.to_string());
        }
    }

    /// The picture is right; the document will not come back.
    fn round_trip(&mut self, what: &str) {
        if !self.round_trip.iter().any(|s| s == what) {
            self.round_trip.push(what.to_string());
        }
    }
}

/// Render a whole page, or a single artboard subtree, to an SVG string.
pub fn svg(doc: &Document, res: &Resolved, page: NodeId, artboard: Option<NodeId>) -> String {
    svg_of(doc, res, &[artboard.unwrap_or(page)])
}

/// Render **these subtrees** to one SVG, in the order given.
///
/// The viewBox covers the union of the world bounds, and each subtree is emitted
/// under its own parent's transform. Order is the caller's and kept (SVG has no
/// z-index; invariant 9). Nothing is deduplicated (`build::outermost` filters).
pub fn svg_of(doc: &Document, res: &Resolved, origins: &[NodeId]) -> String {
    svg_of_reported(doc, res, origins).svg
}

/// [`svg_of`], and what it could not say exactly — see [`Export`]. [`svg_of`] is
/// the markup-only shim; the export panel and *Copy as SVG* ask for the report.
pub fn svg_of_reported(doc: &Document, res: &Resolved, origins: &[NodeId]) -> Export {
    svg_with(doc, res, origins, ExportText::Live)
}

/// [`svg_of_reported`] with text written as `text` asks: `<text>` elements, or the
/// glyph outlines the canvas draws.
pub fn svg_with(doc: &Document, res: &Resolved, origins: &[NodeId], text: ExportText) -> Export {
    svg_linking(doc, res, origins, Default::default(), text, None, false)
}

/// [`svg_with`] with `view`, a world-space box, as the `viewBox`, so whatever of
/// the origins lies outside it is cut off: what a slice exports.
pub fn svg_region(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    view: Rect,
    text: ExportText,
) -> Export {
    svg_linking(
        doc,
        res,
        origins,
        Default::default(),
        text,
        Some(view),
        false,
    )
}

/// [`svg_with`], each picture in `linked` written as a link to its path there
/// rather than embedded: code export's pages share one file per picture. `view`
/// frames the file on a box of its own rather than on the origins' extent.
///
/// `faces` cuts each text run where the face drawing it changes and names the
/// face, for the PDF converter: given the document's family, it draws a whole run
/// in a fallback face that covers it all, Latin included, where the canvas drew
/// only the characters the family lacks. A file someone reads keeps the
/// document's family, which a viewer falls back from by itself, and reads back
/// without a family change per cut.
pub(crate) fn svg_linking(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    linked: std::collections::HashMap<mastik_core::ImageId, String>,
    text: ExportText,
    view: Option<Rect>,
    faces: bool,
) -> Export {
    let style = Style::default();
    svg_framed(doc, res, origins, linked, text, view, faces, false, style)
}

/// [`svg_linking`] with the view's corner moved to the origin, so the markup
/// says nothing of where the origins sit in the world: code export's pictures,
/// where the same icon in two variants must write the same file.
pub(crate) fn svg_at_zero(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    linked: std::collections::HashMap<mastik_core::ImageId, String>,
    text: ExportText,
) -> Export {
    let style = Style {
        credits: false,
        ..Style::default()
    };
    svg_framed(doc, res, origins, linked, text, None, false, true, style)
}

/// The file an export spec asks for of `node`: its own subtree, or a slice's box
/// of its page, framed as [`ExportSpec::view`] says — grown about its centre to a
/// square under [`ExportSpec::pad_square`] — with the box's corner at `0 0`,
/// numbers to [`ExportSpec::precision`] places, and solid paint and strokes as
/// [`ExportSpec::colour`] and [`ExportSpec::strokes`] say.
pub fn svg_spec(doc: &Document, res: &Resolved, node: NodeId, spec: &ExportSpec) -> Export {
    let (origins, view) = match crate::plan::slice_region(doc, res, node) {
        Some((view, origins)) => (origins, Some(view)),
        None => (vec![node], frame_view(doc, res, node, spec.view)),
    };
    let view = match spec.pad_square {
        true => view.or_else(|| crate::extent(res, &origins)).map(|v| {
            let side = v.width().max(v.height());
            Rect::from_center_size(v.center(), (side, side))
        }),
        false => view,
    };
    let style = Style {
        current_color: spec.colour == SvgColour::CurrentColor,
        outline_strokes: spec.strokes == SvgStrokes::Outline,
        precision: spec.precision,
        credits: true,
    };
    let linked = Default::default();
    svg_framed(
        doc, res, &origins, linked, spec.text, view, false, true, style,
    )
}

/// The box [`SvgView::Frame`] frames `node` on: a frame's own world box. `None`
/// for any other layer and under [`SvgView::Artwork`], which take the ink.
pub fn frame_view(doc: &Document, res: &Resolved, node: NodeId, view: SvgView) -> Option<Rect> {
    let framed = matches!(doc.get(node)?.kind(), NodeKind::Artboard { .. });
    match (view, framed) {
        (SvgView::Frame, true) => res.world_bounds(node),
        _ => None,
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "the two public doors' options, passed through"
)]
fn svg_framed(
    doc: &Document,
    res: &Resolved,
    origins: &[NodeId],
    linked: std::collections::HashMap<mastik_core::ImageId, String>,
    text: ExportText,
    view: Option<Rect>,
    faces: bool,
    at_zero: bool,
    style: Style,
) -> Export {
    let _places = PrecisionScope::set(style.precision);
    // `crate::extent`, shared with the PNG writer so the two frame the same box.
    let view = view
        .or_else(|| crate::extent(res, origins))
        .unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));
    // Every region and transform is written relative to an origin's `outer`, so
    // shifting it moves the whole drawing with the view.
    let shift = match at_zero {
        true => Affine::translate(-view.origin().to_vec2()),
        false => Affine::IDENTITY,
    };
    let view = match at_zero {
        true => Rect::from_origin_size((0.0, 0.0), view.size()),
        false => view,
    };
    let origin_outer =
        |doc: &Document, res: &Resolved, id: NodeId| shift * origin_outer(doc, res, id);

    // Defs first so the header can carry them; ids follow document order.
    let mut defs = Defs {
        linked,
        text,
        faces,
        style,
        ..Default::default()
    };
    for id in origins {
        collect_defs(doc, res, *id, origin_outer(doc, res, *id), &mut defs);
    }

    let mut out = String::new();
    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="{x} {y} {w} {h}">"#,
        x = fmt(view.min_x()),
        y = fmt(view.min_y()),
        w = fmt(view.width()),
        h = fmt(view.height()),
    );
    out.push('\n');
    if style.credits {
        credits_metadata(&mut out, doc, origins);
    }
    defs.emit(&mut out);

    let mut report = Report::default();
    for id in origins {
        emit_node(
            &mut out,
            doc,
            res,
            *id,
            origin_outer(doc, res, *id),
            &defs,
            1,
            &mut report,
        );
    }

    out.push_str("</svg>\n");
    Export {
        svg: out,
        approximated: report.approximated,
        round_trip: report.round_trip,
        merged: defs.merged.borrow().len(),
    }
}

/// The `outer` an origin is emitted under — its parent's world transform (identity
/// for the root), since `emit_node` composes `outer * node.transform()`. Shared by
/// the defs and emit passes so a filter region and its element share a space.
fn origin_outer(doc: &Document, res: &Resolved, id: NodeId) -> Affine {
    doc.get(id)
        .and_then(|n| n.parent())
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY)
}

/// Gradient definitions, keyed by the node they came from so element emission
/// can look them up again without re-deriving names.
#[derive(Default)]
struct Defs {
    /// Insertion-ordered, which [`Defs::emit`] needs for byte-stability.
    entries: Vec<(DefKey, String)>,
    /// The same ids as a set, so `add` and `has` are O(1) rather than Θ(D²) overall;
    /// std's `HashSet`, as the win is not worth a hasher dependency.
    seen: std::collections::HashSet<DefKey>,
    /// Pictures written as a link to this path, not embedded ([`svg_linking`]).
    linked: std::collections::HashMap<mastik_core::ImageId, String>,
    /// Not a def: the pass's text mode, kept here because every emitter is
    /// handed the defs.
    text: ExportText,
    /// Not a def: name each run's face as its own file names it
    /// (`ExportRun::face`) rather than the document's family — for the PDF
    /// converter, which looks faces up by those names (see [`svg_linking`]).
    faces: bool,
    /// Not a def: what an export spec asks of the pass ([`svg_spec`]).
    style: Style,
    /// The solid colours `currentColor` stood in for, which [`Export::merged`]
    /// counts. A cell because every emitter holds the defs shared.
    merged: std::cell::RefCell<std::collections::BTreeSet<[u8; 3]>>,
}

/// What an export spec asks of the writer beyond the plain file: solid paint as
/// `currentColor`, strokes as their filled ink, and the decimal places.
#[derive(Clone, Copy)]
struct Style {
    current_color: bool,
    outline_strokes: bool,
    precision: u8,
    /// Write the layers' credits at the top ([`credits_metadata`]). Off for code
    /// export's inline pictures, whose page carries them as a comment: a
    /// `<credits>` element is not one JSX knows, and the markup is hashed to pair
    /// variants.
    credits: bool,
}

impl Default for Style {
    fn default() -> Self {
        Self {
            current_color: false,
            outline_strokes: false,
            precision: mastik_core::export::MAX_PRECISION,
            credits: true,
        }
    }
}

thread_local! {
    /// The decimal places [`fmt`] writes, set for one pass by [`svg_framed`]. A
    /// thread-local because `fmt` is the spelling of every number in the file,
    /// reached from far too many places to hand a parameter to.
    static PRECISION: std::cell::Cell<u8> =
        const { std::cell::Cell::new(mastik_core::export::MAX_PRECISION) };
}

/// Puts [`PRECISION`] back when the pass ends, a panic included.
struct PrecisionScope(u8);

impl PrecisionScope {
    fn set(places: u8) -> Self {
        Self(PRECISION.with(|p| p.replace(places.min(mastik_core::export::MAX_PRECISION))))
    }
}

impl Drop for PrecisionScope {
    fn drop(&mut self) {
        PRECISION.with(|p| p.set(self.0));
    }
}

/// A def's id: a `String` whose equality is counted in tests. Blind spot: a
/// comparison spelled on the inner `String` (`e.0 == id.0`) goes uncounted.
#[derive(Clone, Eq)]
struct DefKey(String);

/// Hashes the inner `String` as `str` does, which makes the `Borrow<str>` lookup
/// in [`Defs::has`] sound.
impl std::hash::Hash for DefKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.0.hash(state);
    }
}

impl PartialEq for DefKey {
    fn eq(&self, other: &Self) -> bool {
        #[cfg(test)]
        DEF_KEY_COMPARISONS.with(|c| c.set(c.get() + 1));
        self.0 == other.0
    }
}

impl std::borrow::Borrow<str> for DefKey {
    fn borrow(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
thread_local! {
    /// How many times two `DefKey`s have been compared on this thread. Plain
    /// backticks: this item is `cfg(test)`.
    static DEF_KEY_COMPARISONS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

impl Defs {
    fn add(&mut self, id: String, body: String) {
        let id = DefKey(id);
        if self.seen.insert(id.clone()) {
            self.entries.push((id, body));
        }
    }

    /// Whether a def of this name was collected; [`paint_attr`] asks before
    /// writing a `url(#…)` for an image.
    fn has(&self, id: &str) -> bool {
        self.seen.contains(id)
    }

    fn emit(&self, out: &mut String) {
        if self.entries.is_empty() {
            return;
        }
        out.push_str("  <defs>\n");
        for (_, body) in &self.entries {
            out.push_str(body);
        }
        out.push_str("  </defs>\n");
    }
}

/// Stable per-node, per-slot gradient id (`grad-<node>-fill-0`); the slot always
/// carries the paint's index.
fn gradient_id(node: NodeId, slot: &str) -> String {
    format!("grad-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// Stable per-node, per-slot pattern id (`img-<node>-fill-0`), spelled like
/// [`gradient_id`].
fn image_id(node: NodeId, slot: &str) -> String {
    format!("img-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The letterbox clip's id, for the one framing that needs one.
fn image_clip_id(node: NodeId, slot: &str) -> String {
    format!("ic-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The adjustment filter's id, for a picture that carries one.
fn adjust_id(node: NodeId, slot: &str) -> String {
    format!("adj-{}-{slot}", node.to_wire().replace(':', "_"))
}

/// The payload's id — keyed by the picture, so N shapes sharing a photograph write
/// one base64 copy. Sanitised, since a non-XML-name id would make the picture
/// silently not draw.
fn payload_id(image: &mastik_core::ImageId) -> String {
    let safe: String = image
        .0
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    format!("pix-{safe}")
}

/// The effect stack's filter id (§5.3a): one per node, the stack being one graph.
fn effect_id(node: NodeId) -> String {
    format!("fx-{}", node.to_wire().replace(':', "_"))
}

/// Stable per-node id for a text node's rail (`rail-<node>`); no slot, because a
/// text node has exactly one rail (`NodeKind::Text::on_path`).
fn rail_id(node: NodeId) -> String {
    format!("rail-{}", node.to_wire().replace(':', "_"))
}

/// The box a node's effect filter may paint in, in the wrapper's user space (the
/// node's parent's, see [`origin_outer`]).
///
/// SVG's default region clips any shadow reaching past a tenth of the layer, so it
/// is written from [`Resolved::ink_bounds`], padded by [`working_pad`].
fn effect_region(
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    effects: &[Keyed<Effect>],
) -> Option<Rect> {
    // `None` drops the filter and keeps the artwork, like `ScenePainter::push_effect_layer`.
    Some(ink_in(res, id, id, transform)? + Insets::uniform(working_pad(effects, transform)))
}

/// `ink_of`'s ink box in the space `outer` maps `frame`'s local space into, the
/// base of every region this writer emits.
///
/// `None` where `frame` is scaled flat: its inverse is `NaN`s, and an unparseable
/// region means the referencing element is not rendered.
fn ink_in(res: &Resolved, ink_of: NodeId, frame: NodeId, outer: Affine) -> Option<Rect> {
    let ink = res.ink_bounds(ink_of)?;
    let world = res.world_transform(frame)?;
    if !mastik_core::build::affine_is_invertible(world) {
        return None;
    }
    Some((outer * world.inverse()).transform_rect_bbox(ink))
}

/// `r` as the ` x y width height` attributes of a region or a `<rect>`.
fn region_attrs(r: Rect) -> String {
    format!(
        r#" x="{}" y="{}" width="{}" height="{}""#,
        fmt(r.x0),
        fmt(r.y0),
        fmt(r.width()),
        fmt(r.height()),
    )
}

/// How far outside the layer this stack's inner shadows and sharpens must be able
/// to look. `EffectKind::escape` is `ZERO` for both, but an inner shadow is cast
/// from everything outside the layer and SVG makes results outside the region
/// transparent, so a region at the ink bounds draws nothing. A sharpen's blur at
/// a region that ends on the layer's edge rings far harder in `resvg` than on the
/// canvas; the pad adds no ink, since the composite clamps alpha below zero away.
///
/// Uniform and generous on purpose: too small silently crops, and the final
/// `feComposite operator="in"` confines an inner shadow. `0.0` with neither.
/// The offset counts as written, through `space` (see [`effect_def`]), which a
/// lean stretches past its layer-space length.
fn working_pad(effects: &[Keyed<Effect>], space: Affine) -> f64 {
    effects
        .iter()
        .filter(|e| e.visible)
        .filter_map(|e| match &e.kind {
            EffectKind::InnerShadow(s) => Some(
                mastik_core::effect::reach(s.blur)
                    + s.spread.abs()
                    + offset_in(space, s.offset).hypot(),
            ),
            EffectKind::Sharpen { amount, radius } if *amount > 0.0 && *radius > 0.0 => {
                Some(mastik_core::effect::reach(*radius))
            }
            _ => None,
        })
        .fold(0.0, f64::max)
}

/// A shadow's layer-space `offset` through `space`'s linear part, as the filter
/// writes it: it turns, mirrors and leans with the layer.
fn offset_in(space: Affine, offset: Vec2) -> Vec2 {
    let [a, b, c, d, _, _] = space.as_coeffs();
    Vec2::new(a * offset.x + c * offset.y, b * offset.x + d * offset.y)
}

/// How many blurred copies [`ramp_graph`] draws a progressive blur from.
const RAMP_COPIES: usize = 3;

/// A progressive layer blur as SVG can draw it, writing `result`: the layer
/// blurred at [`RAMP_COPIES`] radii from the start to the end, each kept where a
/// gradient mask says it is the nearest step, and the copies added up. SVG has
/// no variable blur, so between the steps this is a mix of two blurs where the
/// canvas draws one of the radius between: an approximation, reported.
///
/// The masks are `feImage`s of a gradient laid on the ramp's ends, which are in
/// the layer's own space ([`mastik_core::effect::ramps_on_box`]) and reach the
/// filter's through `space`, as the gradient's transform.
fn ramp_graph(
    out: &mut String,
    graphic: &str,
    start: f64,
    ramp: &mastik_core::BlurRamp,
    region: Rect,
    space: Affine,
    result: &str,
) {
    let last = (RAMP_COPIES - 1) as f64;
    // Enough stops that the ease's curve survives a gradient's straight pieces.
    const STOPS: usize = 9;
    for k in 0..RAMP_COPIES {
        let radius = start + (ramp.end - start) * k as f64 / last;
        let stops: String = (0..STOPS)
            .map(|i| {
                let t = i as f64 / (STOPS - 1) as f64;
                let at = ramp.ease.at(t) * last;
                let weight = (1.0 - (at - k as f64).abs()).max(0.0);
                format!(
                    "<stop offset='{}' stop-color='white' stop-opacity='{}'/>",
                    fmt_unit(t),
                    fmt_unit(weight)
                )
            })
            .collect();
        let [a, b, c, d, e, f] = space.as_coeffs().map(fmt_unit);
        let image = format!(
            "<svg xmlns='http://www.w3.org/2000/svg' viewBox='{} {} {} {}' \
             width='{}' height='{}' preserveAspectRatio='none'><linearGradient id='g' \
             gradientUnits='userSpaceOnUse' x1='{}' y1='{}' x2='{}' y2='{}' \
             gradientTransform='matrix({a} {b} {c} {d} {e} {f})'>{stops}</linearGradient>\
             <rect x='{}' y='{}' width='{}' height='{}' fill='url(#g)'/></svg>",
            fmt(region.x0),
            fmt(region.y0),
            fmt(region.width()),
            fmt(region.height()),
            fmt(region.width()),
            fmt(region.height()),
            fmt(ramp.from.x),
            fmt(ramp.from.y),
            fmt(ramp.to.x),
            fmt(ramp.to.y),
            fmt(region.x0),
            fmt(region.y0),
            fmt(region.width()),
            fmt(region.height()),
        );
        let href = image
            .replace('%', "%25")
            .replace('#', "%23")
            .replace('<', "%3C")
            .replace('>', "%3E");
        let _ = writeln!(
            out,
            "      <feGaussianBlur in=\"{graphic}\" stdDeviation=\"{}\" result=\"{result}b{k}\"/>",
            fmt(mastik_core::effect::deviation(radius)),
        );
        let _ = writeln!(
            out,
            "      <feImage href=\"data:image/svg+xml,{href}\" x=\"{}\" y=\"{}\" width=\"{}\" \
             height=\"{}\" preserveAspectRatio=\"none\" result=\"{result}m{k}\"/>",
            fmt(region.x0),
            fmt(region.y0),
            fmt(region.width()),
            fmt(region.height()),
        );
        let kept = match k {
            0 => result.to_string(),
            _ => format!("{result}k{k}"),
        };
        let _ = writeln!(
            out,
            "      <feComposite in=\"{result}b{k}\" in2=\"{result}m{k}\" operator=\"in\" \
             result=\"{kept}\"/>"
        );
        if k > 0 {
            let _ = writeln!(
                out,
                "      <feComposite in=\"{result}\" in2=\"{kept}\" operator=\"arithmetic\" \
                 k1=\"0\" k2=\"1\" k3=\"1\" k4=\"0\" result=\"{result}\"/>"
            );
        }
    }
}

/// A sharpen's amount as [`effect_def`] writes it, rounded to the export's finest
/// precision.
fn sharpen_written(amount: f64) -> f64 {
    let unit = 10f64.powi(i32::from(mastik_core::export::MAX_PRECISION));
    (amount * unit).round() / unit
}

/// The `<filter>` for one node's effect stack.
///
/// Two stages, as `mastik_render::effects::run`: the layer's own appearance, then
/// every shadow cast from that. `color-interpolation-filters="sRGB"` is mandatory;
/// the `linearRGB` default would differ from the canvas.
///
/// `space` maps the layer's own space into the filter's; its linear part carries
/// each shadow's offset, which turns with the layer as on the canvas.
fn effect_def(name: &str, effects: &[Keyed<Effect>], region: Rect, space: Affine) -> String {
    let mut out = format!(
        "    <filter id=\"{name}\" filterUnits=\"userSpaceOnUse\" \
         color-interpolation-filters=\"sRGB\" x=\"{}\" y=\"{}\" width=\"{}\" height=\"{}\">\n",
        fmt(region.x0),
        fmt(region.y0),
        fmt(region.width()),
        fmt(region.height()),
    );
    // Stage one: what the layer itself looks like, every texture first.
    let mut graphic = "SourceGraphic".to_string();
    for (n, e) in effects.iter().enumerate() {
        if let EffectKind::Texture(t) = &e.kind
            && e.visible
            && t.has_ink()
        {
            let r = format!("fx{n}");
            texture_graph(&mut out, &graphic, t, &r);
            graphic = r;
        }
    }
    for (n, e) in effects.iter().enumerate() {
        if !e.visible {
            continue;
        }
        let r = format!("fx{n}");
        match &e.kind {
            EffectKind::Filters(f) if !f.is_neutral() => {
                let values = mastik_render::effects::filter_matrix(f)
                    .iter()
                    .map(|v| fmt_unit(*v as f64))
                    .collect::<Vec<_>>()
                    .join(" ");
                let _ = writeln!(
                    out,
                    "      <feColorMatrix in=\"{graphic}\" type=\"matrix\" values=\"{values}\" result=\"{r}\"/>"
                );
                graphic = r;
            }
            EffectKind::LayerBlur {
                radius,
                ramp: Some(ramp),
            } if e.kind.is_progressive() => {
                ramp_graph(&mut out, &graphic, *radius, ramp, region, space, &r);
                graphic = r;
            }
            EffectKind::LayerBlur { radius, .. } if *radius > 0.0 => {
                let _ = writeln!(
                    out,
                    "      <feGaussianBlur in=\"{graphic}\" stdDeviation=\"{}\" result=\"{r}\"/>",
                    fmt(mastik_core::effect::deviation(*radius)),
                );
                graphic = r;
            }
            // `effects::sharpen`'s unsharp mask: `k2·layer + k3·blur`, which
            // `feComposite` clamps as the renderers do. The amount is rounded to
            // four places once and both coefficients written from it, so they sum
            // to exactly 1, the shape `svg_in::sharpen_amount` reads back; `1 + a`
            // and `−a` rounded apart need not. An amount that rounds to 0 is left
            // out: the reader refuses a zero `k3`, and with it the whole stack.
            EffectKind::Sharpen { amount, radius }
                if *radius > 0.0 && sharpen_written(*amount) > 0.0 =>
            {
                let amount = &sharpen_written(*amount);
                let _ = writeln!(
                    out,
                    "      <feGaussianBlur in=\"{graphic}\" stdDeviation=\"{}\" result=\"{r}b\"/>",
                    fmt(mastik_core::effect::deviation(*radius)),
                );
                let _ = writeln!(
                    out,
                    "      <feComposite in=\"{graphic}\" in2=\"{r}b\" operator=\"arithmetic\" \
                     k1=\"0\" k2=\"{}\" k3=\"{}\" k4=\"0\" result=\"{r}\"/>",
                    fmt_unit(1.0 + amount),
                    fmt_unit(-amount),
                );
                graphic = r;
            }
            EffectKind::Noise(n) if n.has_ink() => {
                noise_graph(&mut out, &graphic, n, &r);
                graphic = r;
            }
            _ => {}
        }
    }

    // Stage two: the shadows, every one of them cast from `graphic`.
    let mut behind: Vec<String> = Vec::new();
    let mut over: Vec<String> = Vec::new();
    for (n, e) in effects.iter().enumerate() {
        if !e.visible {
            continue;
        }
        let (s, inner) = match &e.kind {
            EffectKind::DropShadow(s) => (s, false),
            EffectKind::InnerShadow(s) => (s, true),
            _ => continue,
        };
        if s.color.components[3] <= 0.0 {
            continue;
        }
        let r = shadow_graph(&mut out, &graphic, s, offset_in(space, s.offset), n, inner);
        if inner { &mut over } else { &mut behind }.push(r);
    }

    if behind.is_empty() && over.is_empty() {
        // A filter with no primitives renders the element as nothing, so a stack
        // whose every entry was skipped would erase the layer; a bare `feOffset`
        // is the shortest "unchanged".
        if graphic == "SourceGraphic" {
            out.push_str("      <feOffset/>\n");
        }
    } else {
        // Bottom to top: drop shadows, the layer, inner shadows — each group in
        // panel order.
        out.push_str("      <feMerge>\n");
        for r in behind.iter().chain(std::iter::once(&graphic)).chain(&over) {
            let _ = writeln!(out, "        <feMergeNode in=\"{r}\"/>");
        }
        out.push_str("      </feMerge>\n");
    }
    out.push_str("    </filter>\n");
    out
}

/// `effects::noise` as primitives over `graphic`, its result named `r`.
///
/// `feTurbulence`; a matrix stretching each channel about mid-grey into an
/// opaque `u`, as `Noise::shade` does; from `u`, the colour laid on (`n`, `u`
/// itself for a multicolour noise) and how much of it shows (`m`'s alpha), cut
/// by the density floor on the turbulence's own alpha. Then the opaque colour
/// blended over the layer and cut to it, mixed with the layer by `m` as `in` and
/// `out` composites summed.
fn noise_graph(out: &mut String, graphic: &str, n: &mastik_core::Noise, r: &str) {
    let n = n.clamped();
    out.push_str(&turbulence(
        n.base_frequency(),
        mastik_core::noise::NOISE_OCTAVES,
        n.seed,
        &format!("{r}t"),
    ));
    let k = f64::from(mastik_core::noise::NOISE_CONTRAST);
    let (k, c) = (fmt_unit(k), fmt_unit(0.5 - 0.5 * k));
    let mut matrix = |input: &str, values: String, result: &str| {
        let _ = writeln!(
            out,
            "      <feColorMatrix in=\"{input}\" type=\"matrix\" values=\"{values}\" result=\"{result}\"/>"
        );
    };
    matrix(
        &format!("{r}t"),
        format!("{k} 0 0 0 {c}  0 {k} 0 0 {c}  0 0 {k} 0 {c}  0 0 0 0 1"),
        &format!("{r}u"),
    );
    let colour = match n.multi {
        true => {
            matrix(
                &format!("{r}u"),
                format!(
                    "0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 0 {}",
                    fmt_unit(n.opacity)
                ),
                &format!("{r}m"),
            );
            format!("{r}u")
        }
        false => {
            let [from, to] = n.colors.map(|c| c.components.map(f64::from));
            let row =
                |i: usize| format!("{} 0 0 0 {}", fmt_unit(to[i] - from[i]), fmt_unit(from[i]));
            matrix(
                &format!("{r}u"),
                format!("{}  {}  {}  0 0 0 0 1", row(0), row(1), row(2)),
                &format!("{r}n"),
            );
            matrix(
                &format!("{r}u"),
                format!("0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  {}", row(3)),
                &format!("{r}m"),
            );
            format!("{r}n")
        }
    };
    let mut strength = format!("{r}m");
    if let Some(floor) = n.density_floor() {
        // A step on the turbulence's alpha: 1 from the floor's level up, 0 below.
        let slope = mastik_core::noise::DENSITY_SLOPE;
        let edge = (f64::from(floor) - 0.5) / 255.0;
        let _ = writeln!(
            out,
            "      <feComponentTransfer in=\"{r}t\" result=\"{r}g\">\n        \
             <feFuncA type=\"linear\" slope=\"{}\" intercept=\"{}\"/>\n      \
             </feComponentTransfer>",
            fmt(slope),
            fmt_fixed(-slope * edge, 4),
        );
        let _ = writeln!(
            out,
            "      <feComposite in=\"{r}m\" in2=\"{r}g\" operator=\"in\" result=\"{r}k\"/>"
        );
        strength = format!("{r}k");
    }
    let _ = writeln!(
        out,
        "      <feBlend in=\"{colour}\" in2=\"{graphic}\" mode=\"{}\" result=\"{r}b\"/>\n      \
         <feComposite in=\"{r}b\" in2=\"{graphic}\" operator=\"in\" result=\"{r}c\"/>\n      \
         <feComposite in=\"{r}c\" in2=\"{strength}\" operator=\"in\" result=\"{r}x\"/>\n      \
         <feComposite in=\"{graphic}\" in2=\"{strength}\" operator=\"out\" result=\"{r}y\"/>\n      \
         <feComposite in=\"{r}x\" in2=\"{r}y\" operator=\"arithmetic\" \
         k1=\"0\" k2=\"1\" k3=\"1\" k4=\"0\" result=\"{r}\"/>",
        n.blend.css(),
    );
}

/// `effects::texture` as primitives over `graphic`, its result named `r`:
/// `feTurbulence` read by `feDisplacementMap`, red across and green down. The
/// filter's `sRGB` interpolation keeps the map's levels as the backends read them.
fn texture_graph(out: &mut String, graphic: &str, t: &mastik_core::Texture, r: &str) {
    let t = t.clamped();
    out.push_str(&turbulence(
        t.base_frequency(),
        u32::from(t.detail),
        t.seed,
        &format!("{r}t"),
    ));
    let _ = writeln!(
        out,
        "      <feDisplacementMap in=\"{graphic}\" in2=\"{r}t\" scale=\"{}\" \
         xChannelSelector=\"R\" yChannelSelector=\"G\" result=\"{r}\"/>",
        fmt(t.scale()),
    );
}

/// Whether `effects` holds a visible noise or texture, whose grain lives in the
/// layer's own space: its filter is then written inside the layer's transform
/// rather than in the parent's space every other stack is written in.
fn grained(effects: &[Keyed<Effect>]) -> bool {
    noised(effects) || textured(effects)
}

/// Whether `effects` holds a visible noise.
fn noised(effects: &[Keyed<Effect>]) -> bool {
    effects
        .iter()
        .any(|e| e.visible && matches!(&e.kind, EffectKind::Noise(n) if n.has_ink()))
}

/// Whether `effects` holds a visible texture.
fn textured(effects: &[Keyed<Effect>]) -> bool {
    effects
        .iter()
        .any(|e| e.visible && matches!(&e.kind, EffectKind::Texture(t) if t.has_ink()))
}

/// One fractal `<feTurbulence>` primitive named `result`, its base frequencies at
/// nine places: they multiply every coordinate the noise is read at, so four
/// would move a large layer's noise by a visible fraction of a cell.
fn turbulence(freq: (f64, f64), octaves: u32, seed: i32, result: &str) -> String {
    let f = |v: f64| fmt_fixed(v, 9);
    let freq = match freq.0 == freq.1 {
        true => f(freq.0),
        false => format!("{} {}", f(freq.0), f(freq.1)),
    };
    format!(
        "      <feTurbulence type=\"fractalNoise\" baseFrequency=\"{freq}\" \
         numOctaves=\"{octaves}\" seed=\"{seed}\" result=\"{result}\"/>\n",
    )
}

/// The primitives for one shadow, returning its result's name; `offset` is the
/// shadow's offset in the filter's space. An inner shadow inverts the silhouette
/// and clips the result back into the layer's alpha.
fn shadow_graph(
    out: &mut String,
    graphic: &str,
    s: &Shadow,
    offset: Vec2,
    n: usize,
    inner: bool,
) -> String {
    let a = format!("fxa{n}");
    let t = format!("fxt{n}");
    // The layer's alpha as a black silhouette — `SourceAlpha`, but from `graphic`
    // since earlier effects may have changed the layer.
    let _ = writeln!(
        out,
        "      <feColorMatrix in=\"{graphic}\" type=\"matrix\" \
         values=\"0 0 0 0 0  0 0 0 0 0  0 0 0 0 0  0 0 0 1 0\" result=\"{a}\"/>"
    );
    let mut src = a.clone();
    if inner {
        // Everything *outside* the layer, which is what an inner shadow casts
        // from. `tableValues="1 0"` is alpha inverted.
        let _ = writeln!(
            out,
            "      <feComponentTransfer in=\"{src}\" result=\"{t}\">\n        \
             <feFuncA type=\"table\" tableValues=\"1 0\"/>\n      </feComponentTransfer>"
        );
        src = t.clone();
    }
    if s.spread != 0.0 {
        // `feMorphology` is a square kernel, so a large spread on a curved shape
        // comes out slightly boxier than on the canvas; it is SVG's only spread
        // primitive, and dropping spread would get the size visibly wrong.
        let op = if (s.spread > 0.0) != inner {
            "dilate"
        } else {
            "erode"
        };
        let m = format!("fxm{n}");
        let _ = writeln!(
            out,
            "      <feMorphology in=\"{src}\" operator=\"{op}\" radius=\"{}\" result=\"{m}\"/>",
            fmt(s.spread.abs()),
        );
        src = m;
    }
    let o = format!("fxo{n}");
    let _ = writeln!(
        out,
        "      <feOffset in=\"{src}\" dx=\"{}\" dy=\"{}\" result=\"{o}\"/>",
        fmt(offset.x),
        fmt(offset.y),
    );
    let mut shape = o;
    if s.blur > 0.0 {
        let b = format!("fxb{n}");
        let _ = writeln!(
            out,
            "      <feGaussianBlur in=\"{shape}\" stdDeviation=\"{}\" result=\"{b}\"/>",
            fmt(mastik_core::effect::deviation(s.blur)),
        );
        shape = b;
    }
    // Colour it: flood the region, cut to the silhouette. `feDropShadow` has no
    // spread and cannot read from `graphic`.
    let f = format!("fxf{n}");
    let c = format!("fxc{n}");
    let _ = writeln!(
        out,
        "      <feFlood flood-color=\"{}\" flood-opacity=\"{}\" result=\"{f}\"/>",
        hex(s.color),
        fmt_unit(s.color.components[3] as f64),
    );
    let _ = writeln!(
        out,
        "      <feComposite in=\"{f}\" in2=\"{shape}\" operator=\"in\" result=\"{c}\"/>"
    );
    if inner {
        // Confine it to the layer, or the inverted silhouette paints outside it.
        let k = format!("fxk{n}");
        let _ = writeln!(
            out,
            "      <feComposite in=\"{c}\" in2=\"{a}\" operator=\"in\" result=\"{k}\"/>"
        );
        k
    } else {
        c
    }
}

/// The paint list's visible entries with their index, which names the gradient
/// slot and alignment clip so a hidden paint does not renumber the rest. Of a paint
/// already resolved (`Document::resolved_paint`); read brushes with `drawn`.
fn visible_fills(paint: &Paint) -> impl Iterator<Item = (usize, &Fill)> {
    paint
        .fills
        .iter()
        .enumerate()
        .filter(|(_, f)| f.visible)
        .map(|(i, f)| (i, &f.value))
}

fn visible_strokes(paint: &Paint) -> impl Iterator<Item = (usize, &Stroke)> {
    paint
        .strokes
        .iter()
        .enumerate()
        .filter(|(_, s)| s.visible)
        .map(|(i, s)| (i, &s.value))
}

fn collect_defs(doc: &Document, res: &Resolved, id: NodeId, outer: Affine, defs: &mut Defs) {
    let Some(node) = doc.get(id) else { return };
    // The same composition `emit_node` makes, so region and element share a space.
    let transform = outer * res.used_local_of(node);
    // As drawn: a bound effect number takes its variable's value, and a ramp's
    // ends are on the layer's box in its own space.
    let effects = doc.resolved_effects(id, node.effects());
    let ramped = effects.iter().any(|e| e.kind.ramp().is_some());
    let effects = match ramped
        .then(|| mastik_core::local_box(doc, res, id))
        .flatten()
    {
        Some(b) => mastik_core::effect::ramps_on_box(effects, b),
        None => effects,
    };
    // A noise's or texture's filter is written in the layer's own space
    // (`emit_node`), and `ink_in` under the identity is that space.
    let space = match grained(&effects) {
        true => Affine::IDENTITY,
        false => transform,
    };
    if mastik_render::effects::any_ink(&effects)
        && let Some(region) = effect_region(res, id, space, &effects)
    {
        defs.add(
            effect_id(id),
            effect_def(&effect_id(id), &effects, region, space),
        );
    }
    // A rail is a def because `<textPath>` can name nothing else; it is in the
    // text node's own space, as the referencing element carries the transform.
    // Outlined glyphs are already bent and name none.
    if let NodeKind::Text {
        on_path: Some(rail),
        on_path_flip,
        ..
    } = node.kind()
        && defs.text == ExportText::Live
    {
        // A flip is a reversed rail, not SVG 2's unevenly implemented
        // `side="right"`: our flip *is* traversing backwards (`PathWarp::flip`).
        let d = if *on_path_flip {
            path_d(&rail.reverse_subpaths())
        } else {
            path_d(rail)
        };
        defs.add(
            rail_id(id),
            format!(
                "    <path id=\"{name}\" d=\"{d}\" fill=\"none\"/>\n",
                name = rail_id(id),
            ),
        );
    }
    // `pad` is how far the slot's ink reaches past the shape's box: a stroke's
    // outer half or whole width, further at a mitred corner or a head.
    let mut consider = |brush: &Brush, slot: &str, pad: f64| {
        match brush {
            Brush::Gradient(g) => {
                let name = gradient_id(id, slot);
                // Only a sweep needs the shape's box, and finding it costs an
                // outline walk; `Linear` and `Radial` carry their own geometry.
                let frame = matches!(g.gradient.kind, GradientKind::Sweep(_))
                    .then(|| paint_frame(node, res, id))
                    .flatten()
                    .map(|f| f.inflate(pad, pad));
                let body = gradient_def(&name, g, frame);
                defs.add(name, body);
            }
            // A `<pattern>`, only when the table can answer; otherwise no def
            // (never a dangling `url(#…)`) and `emit_missing` draws a placeholder.
            Brush::Image(img) => {
                if let (Some(entry), Some(f)) = (
                    doc.image(&img.image.id),
                    framing_of(doc, node, res, id, brush),
                ) {
                    // Adjustments are a `<filter>` def; a neutral picture has none.
                    let filter = img.image.adjust.pipeline().map(|p| {
                        let name = adjust_id(id, slot);
                        defs.add(name.clone(), adjust_def(&name, &p));
                        name
                    });
                    // `Defs::add` dedupes, so the payload is a no-op after the first node.
                    let payload = payload_id(&img.image.id);
                    let link = defs.linked.get(&img.image.id).cloned();
                    defs.add(payload.clone(), image_payload_def(&payload, entry, link));
                    let name = image_id(id, slot);
                    let body = image_def(
                        &name,
                        &payload,
                        entry,
                        f.transform,
                        img.sampler.alpha,
                        filter.as_deref(),
                    );
                    defs.add(name, body);
                }
            }
            Brush::Solid(_) => {}
        }
    };
    // As drawn: gradients on the box layout gives the layer.
    let Some(paint) = mastik_core::paint_box::node_paint(doc, res, id) else {
        return;
    };
    for (i, f) in visible_fills(&paint) {
        consider(f.brush.drawn(), &format!("fill-{i}"), 0.0);
    }
    for (i, s) in visible_strokes(&paint) {
        let outer = match s.align {
            StrokeAlign::Inside => 0.0,
            StrokeAlign::Center => s.width / 2.0,
            StrokeAlign::Outside => s.width,
        };
        let corner = match s.join {
            mastik_core::kurbo::Join::Miter => s.miter_limit.max(1.0),
            _ => 1.0,
        };
        let head = match s.markers().is_none() {
            true => 0.0,
            false => 3.0 * s.marker_size * s.width,
        };
        let pad = (outer * corner).max(head);
        consider(
            s.brush.drawn(),
            &format!("stroke-{i}"),
            if pad.is_finite() { pad } else { 0.0 },
        );
    }
    // Identity for children: the node's own `<g transform>` establishes their
    // space, as in `emit_node`'s recursion.
    for child in node.children() {
        collect_defs(doc, res, *child, Affine::IDENTITY, defs);
    }
}

/// The box an image fill is framed in, as in `scene.rs`. For text it is the
/// layout's box; [`outline`] here would silently frame to the ink.
fn paint_frame(node: &Node, res: &Resolved, id: NodeId) -> Option<Rect> {
    match res.used_kind_of(node) {
        NodeKind::Artboard { size, .. } => Some(Rect::new(0.0, 0.0, size.width, size.height)),
        NodeKind::Text { .. } => res.text_layout(id).map(|l| l.bounds()),
        _ => outline(node, res, id).map(|p| p.bounding_box()),
    }
}

/// How an image `brush` sits in this node's frame; [`None`] for anything else, a
/// missing entry or no frame. Mirrors `scene::framing_of`, taking the intrinsic
/// size from the document's table so nothing need be decoded.
fn framing_of(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    brush: &Brush,
) -> Option<Framing> {
    let Brush::Image(img) = brush else {
        return None;
    };
    let entry = doc.image(&img.image.id)?;
    let frame = paint_frame(node, res, id)?;
    Some(img.image.framing(frame, entry.width, entry.height))
}

/// Whether an image `brush` leaves part of the frame to its pad extend — a crop
/// outside `0..1`, which the canvas fills with the edge pixels and a `<pattern>`
/// with the next repeat. Fit's letterbox is clipped instead, so it does not count.
fn crops_past_picture(
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    brush: &Brush,
) -> bool {
    let Some(f) = framing_of(doc, node, res, id, brush) else {
        return false;
    };
    let (Brush::Image(img), Some(frame)) = (brush, paint_frame(node, res, id)) else {
        return false;
    };
    let Some(entry) = doc.image(&img.image.id) else {
        return false;
    };
    if f.extend != Extend::Pad || f.clip.is_some() {
        return false;
    }
    let source = Rect::new(0.0, 0.0, f64::from(entry.width), f64::from(entry.height));
    let picture = f.transform.transform_rect_bbox(source);
    let tol = 1e-6;
    picture.x0 > frame.x0 + tol
        || picture.y0 > frame.y0 + tol
        || picture.x1 < frame.x1 - tol
        || picture.y1 < frame.y1 - tol
}

/// One image definition: a `<pattern>` in the node's own local space, wrapping
/// the encoded original as an `<image>`.
///
/// The tile is the intrinsic pixel box [`mastik_core::ImageRef::framing`] maps
/// *from*, so the whole framing is the one `patternTransform`. A pattern tiles
/// where the canvas pads, which differs only for a hand-edited crop outside
/// `0..1` ([`crops_past_picture`] reports it); Fit's letterbox is cut by
/// [`with_image_clip`].
///
/// `alpha` is the sampler's (§5.5a); `filter` is an [`adjust_def`] id. Both are
/// the node's, so they sit on the `<use>` of the shared `<image>`
/// ([`payload_id`]), with no `width`/`height` (inert there).
fn image_def(
    name: &str,
    payload: &str,
    entry: &mastik_core::ImageEntry,
    transform: Affine,
    alpha: f32,
    filter: Option<&str>,
) -> String {
    format!(
        "    <pattern id=\"{name}\" patternUnits=\"userSpaceOnUse\" \
         width=\"{w}\" height=\"{h}\"{t}>\n      \
         <use href=\"#{payload}\"{op}{f}/>\n    </pattern>\n",
        w = entry.width,
        h = entry.height,
        t = matrix_attr("patternTransform", transform),
        op = opacity_attr(alpha),
        f = filter.map_or(String::new(), |id| format!(r#" filter="url(#{id})""#)),
    )
}

/// The encoded picture itself, written once per distinct [`mastik_core::ImageId`];
/// the per-node difference stays in the `<pattern>`'s `patternTransform`. `link`
/// is a path to write in place of the picture.
fn image_payload_def(name: &str, entry: &mastik_core::ImageEntry, link: Option<String>) -> String {
    format!(
        "    <image id=\"{name}\" href=\"{href}\" x=\"0\" y=\"0\" \
         width=\"{w}\" height=\"{h}\" preserveAspectRatio=\"none\"/>\n",
        w = entry.width,
        h = entry.height,
        href = escape_xml(&link.unwrap_or_else(|| entry.href())),
    )
}

/// A picture's adjustments as a `<filter>`: a colour matrix, then a tone curve.
///
/// Writes what `ImageAdjust::pipeline` says (§5.5a). Load-bearing: `sRGB`
/// interpolation (the linear default mismatches the canvas) and a region of the
/// element's own box (the default would bleed into neighbouring tiles).
fn adjust_def(name: &str, p: &mastik_core::AdjustPipeline) -> String {
    let mut out = format!(
        "    <filter id=\"{name}\" color-interpolation-filters=\"sRGB\" \
         x=\"0\" y=\"0\" width=\"1\" height=\"1\">\n",
    );
    if let Some(m) = &p.matrix {
        let values = m
            .iter()
            .map(|v| fmt_unit(*v as f64))
            .collect::<Vec<_>>()
            .join(" ");
        let _ = writeln!(
            out,
            "      <feColorMatrix type=\"matrix\" values=\"{values}\"/>"
        );
    }
    if let Some(c) = &p.curve {
        let table = c
            .iter()
            .map(|v| fmt_unit(*v as f64))
            .collect::<Vec<_>>()
            .join(" ");
        out.push_str("      <feComponentTransfer>\n");
        // The same table on three channels and no `feFuncA`: on alpha, a shadow
        // lift would turn a soft edge opaque.
        for ch in ["R", "G", "B"] {
            let _ = writeln!(
                out,
                "        <feFunc{ch} type=\"table\" tableValues=\"{table}\"/>"
            );
        }
        out.push_str("      </feComponentTransfer>\n");
    }
    out.push_str("    </filter>\n");
    out
}

/// Whether `brush` is a picture this writer cannot draw: only a dangling id counts
/// (a link is an `href`; undecodable bytes are the viewer's problem).
fn missing_picture(doc: &Document, brush: &Brush) -> bool {
    match brush {
        Brush::Image(img) => doc.image(&img.image.id).is_none(),
        _ => false,
    }
}

/// The missing-image placeholder as markup, from core's `missing_placeholder` so it
/// cannot drift from `scene::paint_missing`; clipped to the shape's outline.
#[allow(clippy::too_many_arguments)]
fn emit_missing(
    out: &mut String,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    derived: Option<&BezPath>,
    slot: &str,
    pad: &str,
    defs: &Defs,
) {
    let Some(frame) = paint_frame(node, res, id) else {
        return;
    };
    let Some(m) = mastik_core::missing_placeholder(frame) else {
        return;
    };
    let ground = format!(r##" fill="{}""##, hex(m.ground));
    write_element(
        out,
        node,
        res.used_kind_of(node),
        derived,
        pad,
        "",
        &ground,
        "",
        "",
        defs,
    );
    let cid = format!("mi-{}-{slot}", id.to_wire().replace(':', "_"));
    // The shape as a clip, so the rim reads as an inner border and the cross stops
    // at the edge; `outline` rather than the box.
    let Some(shape) = outline(node, res, id) else {
        return;
    };
    emit_missing_ink(out, &m, &shape, &cid, pad);
}

/// The rim and cross of a placeholder, cut to `shape`; shared by a missing picture
/// and an abandoned boolean.
fn emit_missing_ink(
    out: &mut String,
    m: &mastik_core::Missing,
    shape: &BezPath,
    cid: &str,
    pad: &str,
) {
    let stroke = format!(
        r##" fill="none" stroke="{}" stroke-width="{}""##,
        hex(m.ink),
        fmt(m.width),
    );
    let _ = writeln!(
        out,
        r#"{pad}<clipPath id="{cid}"><path d="{}"/></clipPath>"#,
        path_d(shape)
    );
    let _ = writeln!(out, r#"{pad}<g clip-path="url(#{cid})">"#);
    let _ = writeln!(out, r#"{pad}  <path d="{}"{stroke}/>"#, path_d(&m.cross));
    let _ = writeln!(out, r#"{pad}  <path d="{}"{stroke}/>"#, path_d(shape));
    let _ = writeln!(out, "{pad}</g>");
}

/// The placeholder for a boolean whose arithmetic was abandoned, or `false` when
/// this node is not one; the markup half of `scene::paint_shape`'s branch. The
/// ground is a plain `<path>` since the boolean has no outline of its own.
fn emit_failed_boolean(
    out: &mut String,
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    pad: &str,
) -> bool {
    let Some(area) = mastik_core::boolean_placeholder(doc, res, id) else {
        return false;
    };
    let Some(m) = mastik_core::missing_placeholder(area) else {
        return false;
    };
    // Any tolerance: a rectangle's path is four straight lines and flattens to
    // itself.
    let shape = area.to_path(0.1);
    let _ = writeln!(
        out,
        r#"{pad}<path d="{}" fill="{}"/>"#,
        path_d(&shape),
        hex(m.ground)
    );
    let cid = format!("mb-{}", id.to_wire().replace(':', "_"));
    emit_missing_ink(out, &m, &shape, &cid, pad);
    true
}

/// `#rrggbb` for a colour that is always opaque — the placeholder's two.
fn hex(c: Color) -> String {
    let rgba = c.to_rgba8();
    format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
}

/// Emit `body` inside the letterbox clip `framing` asked for, or plainly. Mirrors
/// `scene::clipped`: neither peniko nor SVG has a transparent extend, so the band
/// beside a contained picture must be cut (only `ImageFit::Fit` asks).
fn with_image_clip(
    out: &mut String,
    clip: Option<Rect>,
    id: NodeId,
    slot: &str,
    pad: &str,
    body: impl FnOnce(&mut String, &str),
) {
    let Some(r) = clip else {
        body(out, pad);
        return;
    };
    let cid = image_clip_id(id, slot);
    let _ = writeln!(
        out,
        r#"{pad}<clipPath id="{cid}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
        x = fmt(r.x0),
        y = fmt(r.y0),
        w = fmt(r.width()),
        h = fmt(r.height()),
    );
    let _ = writeln!(out, r#"{pad}<g clip-path="url(#{cid})">"#);
    body(out, &format!("{pad}  "));
    let _ = writeln!(out, "{pad}</g>");
}

/// The radius a sweep's wedges ([`sweep_def`]) must reach: far enough from
/// `center` that the last stop reaches every corner of `frame` (the largest of the
/// four corner distances, since half the diagonal is wrong off-centre).
///
/// In the gradient's own space: the box is pulled back through the brush
/// transform, as `cx`/`cy`/`r` are read before `gradientTransform`. [`None`] for a
/// singular or non-finite transform.
fn sweep_radius(
    center: mastik_core::kurbo::Point,
    frame: Option<Rect>,
    xform: Affine,
) -> Option<f64> {
    let frame = frame?;
    if !mastik_core::build::affine_is_invertible(xform) {
        return None;
    }
    let inv = xform.inverse();
    let r = [
        (frame.x0, frame.y0),
        (frame.x1, frame.y0),
        (frame.x0, frame.y1),
        (frame.x1, frame.y1),
    ]
    .into_iter()
    .map(|(x, y)| (inv * mastik_core::kurbo::Point::new(x, y)).distance(center))
    .fold(0.0_f64, f64::max);
    (r.is_finite() && r > 0.0).then_some(r)
}

/// One gradient definition, in the node's local space. `frame` is the shape's own
/// box there, read only by the sweep arm (a sweep has no radius); [`None`] where
/// nothing paints.
fn gradient_def(name: &str, brush: &mastik_core::GradientBrush, frame: Option<Rect>) -> String {
    let g = &brush.gradient;
    // `gradientTransform` is how an elliptical radial gradient is spelled: the
    // squash lives on the brush. Identity is written as nothing, its default.
    let xform = if brush.transform == mastik_core::kurbo::Affine::IDENTITY {
        String::new()
    } else {
        let [a, b, c, d, e, f] = fmt_matrix(brush.transform);
        format!(r#" gradientTransform="matrix({a} {b} {c} {d} {e} {f})""#)
    };
    // Nothing clamps a descending ramp on load; the canvas clamps via
    // `color::brush_to_backend`, so clamp here too or a browser draws a flat fill.
    // SVG 1.1 mixes in sRGB, so an Oklab ramp goes as the dense stops that draw it,
    // as the canvas does.
    let mut ramp = g.clone();
    mastik_core::image::mix_in_srgb(&mut ramp);
    let stops: String = ramp
        .stops
        .iter()
        .map(|s| {
            let c = s.color.to_alpha_color::<mastik_core::peniko::color::Srgb>();
            let rgba = c.to_rgba8();
            format!(
                "      <stop offset=\"{}\" stop-color=\"#{:02x}{:02x}{:02x}\"{}/>\n",
                fmt_unit(s.offset as f64),
                rgba.r,
                rgba.g,
                rgba.b,
                alpha_attr("stop-opacity", rgba.a),
            )
        })
        .collect();

    // `extend` is `spreadMethod`; dropping it would silently turn a reflected sheen
    // (read in by `svg_in`) into a flat band.
    let spread = match g.extend {
        Extend::Reflect => r#" spreadMethod="reflect""#,
        Extend::Repeat => r#" spreadMethod="repeat""#,
        Extend::Pad => "",
    };

    match &g.kind {
        GradientKind::Linear(p) => format!(
            "    <linearGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">\n{stops}    </linearGradient>\n",
            fmt(p.start.x),
            fmt(p.start.y),
            fmt(p.end.x),
            fmt(p.end.y),
        ),
        GradientKind::Radial(p) => format!(
            "    <radialGradient id=\"{name}\" gradientUnits=\"userSpaceOnUse\"{spread}{xform} cx=\"{}\" cy=\"{}\" r=\"{}\" fx=\"{}\" fy=\"{}\">\n{stops}    </radialGradient>\n",
            fmt(p.end_center.x),
            fmt(p.end_center.y),
            fmt(p.end_radius as f64),
            fmt(p.start_center.x),
            fmt(p.start_center.y),
        ),
        GradientKind::Sweep(p) => sweep_def(name, brush, &ramp, p, frame),
    }
}

/// The widest turn one wedge of [`sweep_def`] spans, so the straight ramp across
/// each reads as the turn.
const WEDGE_MAX: f64 = std::f64::consts::TAU / 72.0;

/// The least turn a wedge reaches under the next, so anti-aliased edges leave no
/// seam far out; [`WEDGE_UNDERLAY`] holds it near the centre.
const WEDGE_OVERLAP: f64 = WEDGE_MAX / 8.0;

/// The least distance, in the gradient's units, a wedge reaches under the next
/// at any radius. A turn alone is under a pixel near the centre, where two edges'
/// partial coverages composite to less than opaque.
const WEDGE_UNDERLAY: f64 = 1.0;

/// A sweep, which SVG 1.1 lacks, as a `<pattern>` of wedges round the centre,
/// each filled with a linear gradient from the colour at its first edge to the
/// colour at its last. Edges fall on every stop and on every turn the ramp
/// repeats, so hard stops stay hard. `ramp` is the gradient mixed into sRGB stops.
///
/// The wedges reach the farthest corner of `frame` ([`sweep_radius`]); with no box
/// or a singular transform nothing paints, so the `50` is unobservable.
fn sweep_def(
    name: &str,
    brush: &mastik_core::GradientBrush,
    ramp: &mastik_core::peniko::Gradient,
    p: &mastik_core::peniko::SweepGradientPosition,
    frame: Option<Rect>,
) -> String {
    use std::f64::consts::{FRAC_PI_2, TAU};
    let stops: Vec<(f64, [f32; 4])> = ramp
        .stops
        .iter()
        .map(|s| {
            let c = s.color.to_alpha_color::<mastik_core::peniko::color::Srgb>();
            (f64::from(s.offset), c.premultiply().components)
        })
        .collect();
    let span = f64::from(p.end_angle - p.start_angle);
    // The ramp parameter a turn `a` past the start reads, after `extend`.
    let t_at = |a: f64| {
        let t = if span > 1e-9 { a / span } else { 0.0 };
        match ramp.extend {
            Extend::Pad => t.clamp(0.0, 1.0),
            Extend::Repeat => t.rem_euclid(1.0),
            Extend::Reflect => 1.0 - (t.rem_euclid(2.0) - 1.0).abs(),
        }
    };
    // Premultiplied, as the renderers mix, between the stops either side of `t`.
    let colour = |t: f64| -> [f32; 4] {
        let Some(first) = stops.first() else {
            return [0.0; 4];
        };
        let last = stops[stops.len() - 1];
        if t <= first.0 {
            return first.1;
        }
        if t >= last.0 {
            return last.1;
        }
        let i = stops.windows(2).position(|w| t <= w[1].0).unwrap_or(0);
        let ((t0, c0), (t1, c1)) = (stops[i], stops[i + 1]);
        let u = if t1 > t0 {
            ((t - t0) / (t1 - t0)) as f32
        } else {
            1.0
        };
        std::array::from_fn(|k| c0[k] + (c1[k] - c0[k]) * u)
    };
    // Every turn a stop sits at, or the ramp ends or turns back at.
    let mut edges = vec![0.0, TAU];
    if span > 1e-9 {
        let mut k = 0.0;
        while k * span < TAU && edges.len() < 4096 {
            edges.extend(
                stops
                    .iter()
                    .map(|(o, _)| {
                        (k + if ramp.extend == Extend::Reflect && k % 2.0 == 1.0 {
                            1.0 - o
                        } else {
                            *o
                        }) * span
                    })
                    .filter(|a| *a > 0.0 && *a < TAU),
            );
            if ramp.extend == Extend::Pad {
                break;
            }
            k += 1.0;
        }
    }
    edges.sort_by(f64::total_cmp);
    edges.dedup_by(|a, b| (*a - *b).abs() < 1e-9);
    let mut wedges = Vec::new();
    for w in edges.windows(2) {
        // Less a hair, so an f32 angle a rounding past a whole count adds no sliver.
        let pieces = ((w[1] - w[0]) / WEDGE_MAX - 1e-4).ceil().max(1.0);
        let step = (w[1] - w[0]) / pieces;
        wedges.extend(
            (0..pieces as usize).map(|i| (w[0] + step * i as f64, w[0] + step * (i + 1) as f64)),
        );
    }
    let r = sweep_radius(p.center, frame, brush.transform).unwrap_or(50.0);
    // At least `WEDGE_UNDERLAY` at the outer edge, as the band below holds it inside.
    let overlap = WEDGE_OVERLAP.max(WEDGE_UNDERLAY / r).min(WEDGE_MAX);
    // Far enough that a wedge's straight outer edge stays past the circle.
    let reach = r / ((WEDGE_MAX + overlap) / 2.0).cos() * 1.01;
    let (cx, cy, start) = (p.center.x, p.center.y, f64::from(p.start_angle));
    let towards = |a: f64, d: f64| (cx + d * (start + a).cos(), cy + d * (start + a).sin());
    let at = |a: f64| towards(a, reach);
    // Wedges overlap, so a translucent one would paint the overlap twice: then
    // the wedges are drawn opaque and the opacity comes from a luminance mask of
    // the same wedges in opaque greys, which overlap as harmlessly.
    let translucent = stops.iter().any(|(_, c)| c[3] < 1.0);
    let rgba = |c: [f32; 4]| {
        mastik_core::peniko::color::PremulColor::<mastik_core::peniko::color::Srgb>::new(c)
            .un_premultiply()
            .to_rgba8()
    };
    let stop = |offset: &str, c: [f32; 4]| {
        let c = rgba(c);
        let alpha = match translucent {
            true => String::new(),
            false => alpha_attr("stop-opacity", c.a),
        };
        format!(
            "      <stop offset=\"{offset}\" stop-color=\"#{:02x}{:02x}{:02x}\"{alpha}/>\n",
            c.r, c.g, c.b,
        )
    };
    let grey = |offset: &str, c: [f32; 4]| {
        let a = rgba(c).a;
        format!("      <stop offset=\"{offset}\" stop-color=\"#{a:02x}{a:02x}{a:02x}\"/>\n")
    };
    let mut out = String::new();
    let mut paths = String::new();
    let mut greys = String::new();
    // An edge's colour is read just inside the wedge, so a hard stop on it keeps
    // both its sides.
    const INSIDE: f64 = 1e-9;
    for (i, &(a0, a1)) in wedges.iter().enumerate() {
        let ((x0, y0), (x1, y1)) = (at(a0), at(a1));
        let _ = write!(
            out,
            "    <linearGradient id=\"{name}-w{i}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">\n{}{}    </linearGradient>\n",
            fmt(x0),
            fmt(y0),
            fmt(x1),
            fmt(y1),
            stop("0", colour(t_at(a0 + INSIDE))),
            stop("1", colour(t_at(a1 - INSIDE))),
        );
        if translucent {
            let _ = write!(
                out,
                "    <linearGradient id=\"{name}-a{i}\" gradientUnits=\"userSpaceOnUse\" x1=\"{}\" y1=\"{}\" x2=\"{}\" y2=\"{}\">\n{}{}    </linearGradient>\n",
                fmt(x0),
                fmt(y0),
                fmt(x1),
                fmt(y1),
                grey("0", colour(t_at(a0 + INSIDE))),
                grey("1", colour(t_at(a1 - INSIDE))),
            );
        }
        // Each wedge reaches under the next, but never past the seam, where it
        // would paint its colour over the start's. The first reaches back under
        // the last instead, which is painted over it, so the seam is covered too;
        // its gradient pads to its first colour there.
        let end = (a1 + overlap).min(TAU);
        let begin = if i == 0 { a0 - overlap } else { a0 };
        // Near the centre the turn is under a pixel, so the wedge also takes a
        // band `WEDGE_UNDERLAY` wide past its last edge: a corner on that band's
        // line, on the ray a quarter turn on, or on the seam's if that is nearer.
        // The first takes one behind its first edge too, under the last.
        let mut corners = vec![(cx, cy)];
        if i == 0 {
            corners.push(towards(a0 - FRAC_PI_2, WEDGE_UNDERLAY));
        }
        corners.extend([at(begin), at(end)]);
        if end < TAU {
            let side = (a1 + FRAC_PI_2).min(TAU);
            corners.push(towards(
                side,
                (WEDGE_UNDERLAY / (side - a1).sin()).min(reach),
            ));
        }
        let d = corners
            .iter()
            .map(|(x, y)| format!("{} {}", fmt(*x), fmt(*y)))
            .collect::<Vec<_>>()
            .join("L");
        let _ = writeln!(
            paths,
            "      <path d=\"M{d}Z\" fill=\"url(#{name}-w{i})\"/>"
        );
        if translucent {
            let _ = writeln!(
                greys,
                "      <path d=\"M{d}Z\" fill=\"url(#{name}-a{i})\"/>"
            );
        }
    }
    let xform = if brush.transform == Affine::IDENTITY {
        String::new()
    } else {
        let [a, b, c, d, e, f] = fmt_matrix(brush.transform);
        format!(r#" patternTransform="matrix({a} {b} {c} {d} {e} {f})""#)
    };
    // A tile's content counts from the tile's corner; the `viewBox` puts it back in
    // the gradient's own space.
    let (x, y, side) = (fmt(cx - reach), fmt(cy - reach), fmt(reach * 2.0));
    if translucent {
        let _ = write!(
            out,
            "    <mask id=\"{name}-m\" maskUnits=\"userSpaceOnUse\" x=\"{x}\" y=\"{y}\" width=\"{side}\" height=\"{side}\" color-interpolation=\"sRGB\">\n{greys}    </mask>\n",
        );
        paths = format!("      <g mask=\"url(#{name}-m)\">\n{paths}      </g>\n");
    }
    let _ = write!(
        out,
        "    <pattern id=\"{name}\" patternUnits=\"userSpaceOnUse\"{xform} x=\"{x}\" y=\"{y}\" width=\"{side}\" height=\"{side}\" viewBox=\"{x} {y} {side} {side}\">\n{paths}    </pattern>\n",
    );
    out
}

/// Every way this writer is about to say something inexactly about `node`.
///
/// Read off the node once per emitted node, so it restates each rule and
/// `tests/fidelity.rs` checks the report against the markup. `effects` is the
/// stack as drawn (`Document::resolved_effects`).
fn report_node(
    res: &Resolved,
    node: &Node,
    effects: &[mastik_core::Keyed<Effect>],
    filter_written: bool,
    // Text goes out as outlines ([`ExportText::Outlines`]).
    outlined: bool,
    report: &mut Report,
) {
    // `gradient_def` writes a sweep as a pattern of wedges, which no reader takes
    // back as one gradient and which blends towards the centre.
    let sweep = |brush: &Brush| {
        matches!(
            brush,
            Brush::Gradient(g) if matches!(g.gradient.kind, GradientKind::Sweep(_))
        )
    };
    let picture = |brush: &Brush| matches!(brush, Brush::Image(_));
    // Unresolved: a bound paint is a solid, so `drawn` is still right.
    let brushes = || {
        visible_fills(node.paint())
            .map(|(_, f)| f.brush.drawn())
            .chain(visible_strokes(node.paint()).map(|(_, s)| s.brush.drawn()))
    };
    if brushes().any(sweep) {
        report.approximate("a sweep gradient, written as wedges of linear ones");
    }
    if brushes().any(picture) {
        // Not "approximated": the picture lands where the document puts it; only
        // no shipped reader can read a `<pattern>` back.
        report.round_trip("an image fill, written as a pattern this app cannot read back");
    }
    // `resolved_dashes` keeps a round dot as a zero-length dash, which SVG draws;
    // a butt or square one is written as a dash one width long.
    let squared_dot = visible_strokes(node.paint()).any(|(_, s)| {
        s.cap != mastik_core::kurbo::Cap::Round && s.dashes.iter().any(|d| *d <= 0.0)
    });
    if squared_dot {
        report.round_trip("a square-dotted stroke, written as short dashes");
    }
    // Square-kernel `feMorphology` (see `shadow_graph`), only if it is in the file:
    // nonzero spread, visible effect, and the `<filter>` written.
    if filter_written
        && effects.iter().any(|e| {
            e.visible
                && match e.kind {
                    EffectKind::DropShadow(ref s) | EffectKind::InnerShadow(ref s) => {
                        s.spread != 0.0
                    }
                    _ => false,
                }
        })
    {
        report.approximate("a shadow's spread, drawn with a square kernel");
    }
    // A filter reads only its own element, never what lies beneath it, so a
    // shadow's mode has nothing to blend with there.
    if filter_written
        && effects.iter().any(|e| {
            e.visible
                && match e.kind {
                    EffectKind::DropShadow(ref s) | EffectKind::InnerShadow(ref s) => s.blends(),
                    _ => false,
                }
        })
    {
        report.approximate("a shadow's blend mode, drawn normal");
    }
    // `svg_in` reads no `feTurbulence`, and a filter it cannot hold whole it
    // skips whole.
    if filter_written && noised(effects) {
        report.round_trip(
            "a noise, which this app does not read back, nor the rest of its effects with it",
        );
    }
    if filter_written && textured(effects) {
        report.round_trip(
            "a texture, which this app does not read back, nor the rest of its effects with it",
        );
    }
    // SVG has no variable blur: [`ramp_graph`] draws steps of it, through
    // `feImage`, which `svg_in` reads no more than `feTurbulence`.
    let progressive = |e: &&Keyed<Effect>| e.visible && e.kind.is_progressive();
    if filter_written
        && effects
            .iter()
            .filter(progressive)
            .any(|e| matches!(e.kind, EffectKind::LayerBlur { .. }))
    {
        report.approximate("a progressive blur, drawn in three steps");
        report.round_trip(
            "a progressive blur, which this app does not read back, nor the rest of its \
             effects with it",
        );
    }
    // SVG has no backdrop: `BackgroundImage` was never implemented by any viewer.
    if mastik_core::effect::backdrop_radius(effects).is_some() {
        let ramped = effects
            .iter()
            .filter(progressive)
            .any(|e| matches!(e.kind, EffectKind::BackgroundBlur { .. }));
        report.approximate(match ramped {
            true => "a progressive background blur, left out",
            false => "a background blur, left out",
        });
    }
    if mastik_core::effect::glasses(effects).next().is_some() {
        report.approximate("a glass, left out");
    }
    // Everything below is about `<text>`; outlined text is our own glyphs, placed.
    if outlined {
        return;
    }
    if let NodeKind::Text {
        on_path: Some(_),
        on_path_flip,
        ..
    } = node.kind()
    {
        // A `<textPath>`'s glyph positions along the rail are the viewer's.
        report.approximate("type on a path, whose glyph placement is left to the viewer");
        if *on_path_flip {
            // Not an approximation: a reversed `d` is the identical drawing (see
            // `collect_defs`); `svg_in` reads it back with the flag clear.
            report.round_trip("flipped type on a path, written as a reversed rail");
        }
    }
    // `run_attrs` writes these four; the reader takes none of them back. The used
    // kind, as `run_attrs` reads: a bound size's stored literal is 0.
    if let Some(parts) = mastik_core::TextRef::of(res.used_kind_of(node)) {
        let runs = parts.spans.runs(parts.style, parts.content.len());
        let any = |f: &dyn Fn(&mastik_core::TextStyle) -> bool| runs.iter().any(|(_, s)| f(s));
        if any(&|s| !s.variations.is_empty()) {
            report.round_trip("font variation settings, which this app does not read back");
        }
        if any(&|s| !s.features.is_empty()) {
            report.round_trip("OpenType features, which this app does not read back");
        }
        if any(&|s| s.baseline_shift.resolve(s.font_size) != 0.0) {
            report.round_trip("a baseline shift, which this app does not read back");
        }
        if any(&|s| s.locale.is_some()) {
            report.round_trip("a text language, which this app does not read back");
        }
    }
}

/// Emit a node (if visible) and recurse into its children in z-order.
///
/// Containers become `<g>` elements so their transform and opacity nest the way
/// the model means them to. `outer` is an extra transform folded into this
/// node's own — only non-identity for the root of a single-artboard export.
// `defs` and `report` are related only by being pass-wide; no struct to please the lint.
#[allow(clippy::too_many_arguments)]
fn emit_node(
    out: &mut String,
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    outer: Affine,
    defs: &Defs,
    depth: usize,
    report: &mut Report,
) {
    let Some(node) = doc.get(id) else { return };
    // Shown, a bound visibility resolved. A slice is never drawn.
    if !doc.shown(id) || matches!(node.kind(), NodeKind::Slice { .. }) {
        return;
    }
    // The effect stack gets a transform-free wrapper, not an attribute: `filter`
    // applies before `clip-path` (a frame's shadow would be cut), and a shape may be
    // several elements.
    let drawn_fx = doc.resolved_effects(id, node.effects());
    let fx = mastik_render::effects::any_ink(&drawn_fx) && defs.has(&effect_id(id));
    // Under the visibility guard: the report is a claim about the file.
    report_node(
        res,
        node,
        &drawn_fx,
        fx,
        defs.text == ExportText::Outlines,
        report,
    );
    let paint = doc.resolved_paint(id, node.paint());
    if visible_fills(&paint).any(|(_, f)| crops_past_picture(doc, node, res, id, f.brush.drawn())) {
        report
            .approximate("a crop past the picture's edge, repeated where the canvas stretches it");
    }
    // The blend mode is the outermost wrapper, outside even the filter, as the
    // scene walk's blend layer is: a layer's shadow blends with it.
    let blend = blend_style(doc, node);
    let blend_pad = "  ".repeat(depth);
    let depth = depth + usize::from(blend.is_some());
    if let Some(style) = &blend {
        let _ = writeln!(out, "{blend_pad}<g style=\"{style}\">");
    }
    let wrap_pad = "  ".repeat(depth);
    let depth = depth + usize::from(fx);
    // A stack with noise or texture is filtered in the layer's own space, where
    // the grain lives: the wrapper takes the layer's transform and undoes it
    // inside, so the artwork lands where it always does.
    let own = outer * res.used_local_of(node);
    let local = fx && grained(&drawn_fx) && mastik_core::build::affine_is_invertible(own);
    if local {
        let _ = writeln!(
            out,
            "{wrap_pad}<g{}><g filter=\"url(#{})\"><g{}>",
            transform_attr(own),
            effect_id(id),
            transform_attr(own.inverse()),
        );
    } else if fx {
        let _ = writeln!(out, "{wrap_pad}<g filter=\"url(#{})\">", effect_id(id));
    }
    // Closes the filter wrapper and the blend one, in that order.
    let close_fx = |out: &mut String| {
        if local {
            let _ = writeln!(out, "{wrap_pad}</g></g></g>");
        } else if fx {
            let _ = writeln!(out, "{wrap_pad}</g>");
        }
        if blend.is_some() {
            let _ = writeln!(out, "{blend_pad}</g>");
        }
    };

    let pad = "  ".repeat(depth);
    // Used geometry from here down; what sort of node this is stays on `node.kind()`.
    let transform = outer * res.used_local_of(node);
    // As drawn, a bound opacity resolved.
    let opacity = doc.drawn_opacity(node.id());

    let is_container = matches!(
        node.kind(),
        NodeKind::Root | NodeKind::Group | NodeKind::Artboard { .. }
    );

    if !is_container {
        emit_shape(out, doc, node, res, id, transform, opacity, defs, &pad);
        close_fx(out);
        return;
    }

    // Root carries nothing of its own; skip its wrapper to keep output tidy.
    let wrap = !matches!(node.kind(), NodeKind::Root);
    let paint = mastik_core::paint_box::node_paint(doc, res, id)
        .unwrap_or_else(|| doc.resolved_paint(id, node.paint()));
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(&paint).collect();
    // A frame's stroke cannot live inside the group that clips its contents, so a
    // clip plus a stroke makes an outer group (transform, opacity) holding an inner
    // `clip-path` group, then the stroke.
    let split_clip = wrap && !strokes.is_empty() && node.clip();
    // Where the contents sit: one level in, and one more under a clip group.
    let content_depth = depth + usize::from(wrap) + usize::from(split_clip);
    let content_pad = "  ".repeat(content_depth);
    // Where the frame's own border sits: inside the outer group, beside the clip one.
    let own_pad = "  ".repeat(depth + usize::from(wrap));
    if wrap {
        let clip = match res.used_kind_of(node) {
            kind @ NodeKind::Artboard { .. } if node.clip() => {
                let cid = format!("clip-{}", id.to_wire().replace(':', "_"));
                // The rounded outline, as the canvas clips to it.
                let (tag, attrs) = box_shape(kind);
                let _ = writeln!(
                    out,
                    "{pad}<clipPath id=\"{cid}\"><{tag}{attrs}/></clipPath>",
                );
                format!(r#" clip-path="url(#{cid})""#)
            }
            _ => String::new(),
        };
        // The name goes on the outer `<g>` in the split case; the inner one only
        // holds the clip.
        if split_clip {
            let _ = writeln!(
                out,
                "{pad}<g{}{}{}>",
                name_attr(node),
                transform_attr(transform),
                opacity_attr(opacity),
            );
            let _ = writeln!(out, "{own_pad}<g{clip}>");
        } else {
            let _ = writeln!(
                out,
                "{pad}<g{}{}{}{}>",
                name_attr(node),
                transform_attr(transform),
                opacity_attr(opacity),
                clip,
            );
        }
    }

    // A frame's fills come first inside its group, behind its children; strokes
    // follow via `emit_strokes`, outside the clip. Each is a fill like any other
    // (letterbox clip, `emit_missing` placeholder). The slot `fill-{i}` must match
    // what `collect_defs` wrote.
    if let kind @ NodeKind::Artboard { .. } = res.used_kind_of(node) {
        let (tag, attrs) = box_shape(kind);
        for (i, f) in visible_fills(&paint) {
            let slot = format!("fill-{i}");
            if missing_picture(doc, f.brush.drawn()) {
                emit_missing(out, node, res, id, None, &slot, &content_pad, defs);
                continue;
            }
            let clip = framing_of(doc, node, res, id, f.brush.drawn()).and_then(|f| f.clip);
            with_image_clip(out, clip, id, &slot, &content_pad, |out, pad| {
                let _ = writeln!(
                    out,
                    "{pad}<{tag}{attrs}{fill}/>",
                    fill = paint_attr("fill", f.brush.drawn(), id, &slot, defs),
                );
            });
        }
    }

    // A mask becomes a `<g clip-path>` over the run of siblings above it (as
    // `scene::MaskRun`), emitting no artwork of its own. No transform: the space is
    // this node's, which `mask_path` answers in. No `clip-rule`: non-zero is both
    // SVG's default and the fill rule.
    let mut masked = false;
    let mut child_depth = content_depth;
    for child in node.children() {
        if let Some(child_node) = doc.get(*child)
            && child_node.mask()
        {
            if masked {
                let _ = writeln!(out, "{content_pad}</g>");
                masked = false;
                child_depth = content_depth;
            }
            let mid = format!("mask-{}", child.to_wire().replace(':', "_"));
            // An inverted mask's `<mask>` covers the run: this node's own ink,
            // inflated as an ink mask's region is.
            let run_region = || {
                ink_in(res, id, id, Affine::IDENTITY)
                    .map(|r| region_attrs(r.inflate(r.width() * 0.1, r.height() * 0.1)))
            };
            match child_node.mask_mode() {
                // Inverted, a shape mask is a white cover with the outline painted
                // black, read by luminance: the run survives outside the outline.
                MaskMode::Shape if child_node.mask_inverted() => {
                    if let Some(path) = res.mask_path(doc, *child)
                        && let Some(region) = run_region()
                    {
                        let _ = writeln!(
                            out,
                            r##"{content_pad}<mask id="{mid}" maskUnits="userSpaceOnUse"{region} style="mask-type:luminance"><rect{region} fill="#fff"/><path d="{}" fill="#000"/></mask>"##,
                            path_d(&path),
                        );
                        let _ = writeln!(out, r#"{content_pad}<g mask="url(#{mid})">"#);
                        masked = true;
                        child_depth = content_depth + 1;
                    }
                }
                // A shape mask is a `<clipPath>`: a hard region, no compositing.
                MaskMode::Shape => {
                    // [`Resolved::mask_path`]'s `None` masks nothing (a hidden
                    // mask, an empty text node, a group with nothing in it, a
                    // boolean whose operands cancel), so the run carries on
                    // unwrapped, as on the canvas.
                    if let Some(path) = res.mask_path(doc, *child) {
                        let _ = writeln!(
                            out,
                            r#"{content_pad}<clipPath id="{mid}"><path d="{}"/></clipPath>"#,
                            path_d(&path),
                        );
                        let _ = writeln!(out, r#"{content_pad}<g clip-path="url(#{mid})">"#);
                        masked = true;
                        child_depth = content_depth + 1;
                    }
                }
                // An ink mask is a `<mask>` holding the layer's own markup, since
                // what masks is its ink. `mask-type` is written either way, as CSS
                // for better support; for alpha it is load-bearing (SVG defaults
                // to luminance). An ink-less mask erases the run, as on the canvas.
                //
                // Inverted, the layer's markup goes through a filter that floods
                // white and takes the ink out of it (by alpha, or by brightness
                // times alpha), read as an alpha mask covering the run.
                mode @ (MaskMode::Alpha | MaskMode::Luminance)
                    if doc.shown(*child) && child_node.mask_inverted() =>
                {
                    let Some(region) = run_region() else {
                        continue;
                    };
                    // `luminanceToAlpha` reads unpremultiplied colour, so the
                    // result is taken `in` the source to weigh it by alpha.
                    let (ink, taken) = match mode {
                        MaskMode::Luminance => (
                            r#"<feColorMatrix in="SourceGraphic" type="luminanceToAlpha"/><feComposite in2="SourceGraphic" operator="in" result="ink"/>"#,
                            "ink",
                        ),
                        _ => ("", "SourceGraphic"),
                    };
                    let _ = writeln!(
                        out,
                        r##"{content_pad}<filter id="{mid}-out" filterUnits="userSpaceOnUse"{region} color-interpolation-filters="sRGB">{ink}<feFlood flood-color="#fff"/><feComposite in2="{taken}" operator="out"/></filter>"##,
                    );
                    let _ = writeln!(
                        out,
                        r#"{content_pad}<mask id="{mid}" maskUnits="userSpaceOnUse"{region} style="mask-type:alpha"><g filter="url(#{mid}-out)">"#,
                    );
                    emit_node(
                        out,
                        doc,
                        res,
                        *child,
                        Affine::IDENTITY,
                        defs,
                        content_depth + 1,
                        report,
                    );
                    let _ = writeln!(out, "{content_pad}</g></mask>");
                    let _ = writeln!(out, r#"{content_pad}<g mask="url(#{mid})">"#);
                    report.round_trip(
                        "an inverted alpha or luminance mask, which this app reads back the right way out",
                    );
                    masked = true;
                    child_depth = content_depth + 1;
                }
                // The `doc.shown` guard: a hidden mask would leave an empty `<mask>`
                // and erase the whole run.
                mode @ (MaskMode::Alpha | MaskMode::Luminance) if doc.shown(*child) => {
                    let mask_type = match mode {
                        MaskMode::Luminance => "luminance",
                        _ => "alpha",
                    };
                    // The region is written ([`ink_in`] inflated by a tenth):
                    // `<mask>`'s default is relative to the viewport and erases
                    // everything outside it.
                    let region = ink_in(res, *child, id, Affine::IDENTITY);
                    // No ink erases the run anywhere; no region is needed.
                    let region = region.map_or_else(String::new, |r| {
                        region_attrs(r.inflate(r.width() * 0.1, r.height() * 0.1))
                    });
                    let _ = writeln!(
                        out,
                        r#"{content_pad}<mask id="{mid}" maskUnits="userSpaceOnUse"{region} style="mask-type:{mask_type}">"#,
                    );
                    emit_node(
                        out,
                        doc,
                        res,
                        *child,
                        Affine::IDENTITY,
                        defs,
                        content_depth + 1,
                        report,
                    );
                    let _ = writeln!(out, "{content_pad}</mask>");
                    let _ = writeln!(out, r#"{content_pad}<g mask="url(#{mid})">"#);
                    masked = true;
                    child_depth = content_depth + 1;
                }
                // A hidden mask erases nothing: no wrapper, `masked` stays false.
                MaskMode::Alpha | MaskMode::Luminance => {}
            }
            continue;
        }
        emit_node(
            out,
            doc,
            res,
            *child,
            Affine::IDENTITY,
            defs,
            child_depth,
            report,
        );
    }
    if masked {
        let _ = writeln!(out, "{content_pad}</g>");
    }

    if split_clip {
        let _ = writeln!(out, "{own_pad}</g>");
    }
    // The frame's own border, last inside its group so it covers the contents.
    if !strokes.is_empty() {
        emit_strokes(out, doc, node, res, id, &own_pad, &strokes, defs);
    }

    if wrap {
        let _ = writeln!(out, "{pad}</g>");
    }
    close_fx(out);
}

/// A shape and its paints.
///
/// One plain element when the node has at most one fill and one centred stroke;
/// anything else emits the shape once per paint via [`emit_stacked`].
#[allow(clippy::too_many_arguments)]
fn emit_shape(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    opacity: f32,
    defs: &Defs,
    pad: &str,
) {
    // Before the paints, which it replaces, as in `scene::paint_shape`.
    if emit_failed_boolean(out, doc, res, id, pad) {
        return;
    }
    if defs.text == ExportText::Outlines && matches!(node.kind(), NodeKind::Text { .. }) {
        emit_outlined_text(out, doc, node, res, id, transform, opacity, defs, pad);
        return;
    }
    let paint = mastik_core::paint_box::node_paint(doc, res, id)
        .unwrap_or_else(|| doc.resolved_paint(id, node.paint()));
    let fills: Vec<(usize, &Fill)> = visible_fills(&paint).collect();
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(&paint).collect();
    let derived = res.boolean_path(id);
    let shape = outline(node, res, id);
    // A letterboxing image forces stacking, as an aligned stroke does: the clip
    // belongs to one paint's element.
    let letterboxed = fills
        .iter()
        .map(|(_, f)| f.brush.drawn())
        .chain(strokes.iter().map(|(_, s)| s.brush.drawn()))
        .any(|b| framing_of(doc, node, res, id, b).is_some_and(|f| f.clip.is_some()));
    // So does a missing picture: the placeholder is several elements and a clip.
    let missing = fills
        .iter()
        .any(|(_, f)| missing_picture(doc, f.brush.drawn()));
    let plain = !letterboxed
        && !missing
        && fills.len() <= 1
        && match strokes.as_slice() {
            [] => true,
            // Outlined, a stroke is an element of its own (`emit_strokes`).
            _ if defs.style.outline_strokes => false,
            // A per-side stroke or a fitted multi-subpath dash is several
            // `<path>`s, so neither can ride the shape's own element.
            [(_, s)] => {
                align_clip(res.used_kind_of(node), shape.as_ref(), s, node.fill_rule()).is_none()
                    && geometry::effective_sides(res.used_kind_of(node), s).is_all()
                    && dash_pieces(shape.as_ref(), s, s.width).is_none()
                    && (fills.is_empty()
                        || trimmed_stroke(res.used_kind_of(node), shape.as_ref(), s, Markers::NONE)
                            .is_none())
                    // Its markers are elements of their own (`emit_strokes`).
                    && shape.as_ref().is_none_or(|shape| {
                        geometry::placed_markers(res.used_kind_of(node), shape, s.width, s.markers())
                            .is_empty()
                    })
            }
            _ => false,
        };
    if !plain {
        emit_stacked(
            out, doc, node, res, id, transform, opacity, pad, &fills, &strokes, defs,
        );
        return;
    }

    // The name rides in front of the transform ([`name_attr`]).
    let t = format!("{}{}", name_attr(node), transform_attr(transform));
    let op = opacity_attr(opacity);
    let fill = match fills.first() {
        Some((i, f)) => paint_attr("fill", f.brush.drawn(), id, &format!("fill-{i}"), defs),
        // Text with no ink at all is black, as in the scene walk; with a stroke
        // the letters stay hollow.
        None if matches!(node.kind(), NodeKind::Text { .. }) && strokes.is_empty() => {
            solid_attr("fill", Color::BLACK, defs)
        }
        None => r#" fill="none""#.to_string(),
    };
    // `fill-rule` rides with the fill, even when it is `none` (see `with_fill_rule`).
    let fill = with_fill_rule(fill, node);
    // An unfilled element is all stroke, so it can carry the stroke's cut path.
    let cut = strokes.first().and_then(|(_, s)| {
        trimmed_stroke(res.used_kind_of(node), shape.as_ref(), s, Markers::NONE)
    });
    let stroke = match strokes.first() {
        Some((i, s)) => stroke_attr(
            id,
            *i,
            s,
            s.width,
            &resolved_dashes(cut.as_ref().or(shape.as_ref()), s, s.width),
            defs,
        ),
        None => String::new(),
    };
    write_element(
        out,
        node,
        res.used_kind_of(node),
        cut.as_ref().or(derived),
        pad,
        &t,
        &fill,
        &stroke,
        &op,
        defs,
    );
}

/// A text node as the glyph outlines the canvas draws ([`ExportText::Outlines`]),
/// in `scene::paint_shape`'s text order: each run in its own colour or under every
/// fill, the strokes along the whole outline, then the decoration bands.
#[allow(clippy::too_many_arguments)]
fn emit_outlined_text(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    opacity: f32,
    defs: &Defs,
    pad: &str,
) {
    let Some(layout) = res.text_layout(id) else {
        return;
    };
    let paint = mastik_core::paint_box::node_paint(doc, res, id)
        .unwrap_or_else(|| doc.resolved_paint(id, node.paint()));
    let fills: Vec<(usize, &Fill)> = visible_fills(&paint).collect();
    let strokes: Vec<(usize, &Stroke)> = visible_strokes(&paint).collect();
    // Black only for text with no ink at all; with a stroke the letters stay hollow.
    let black = Brush::Solid(Color::BLACK);
    let inks: Vec<(String, &Brush)> = match fills.is_empty() && strokes.is_empty() {
        true => vec![(String::new(), &black)],
        false => fills
            .iter()
            .map(|(i, f)| (format!("fill-{i}"), f.brush.drawn()))
            .collect(),
    };
    let inner = format!("{pad}  ");
    let _ = writeln!(
        out,
        "{pad}<g{}{}{}>",
        name_attr(node),
        transform_attr(transform),
        opacity_attr(opacity)
    );
    let fill_path = |out: &mut String, path: &BezPath, brush: &Brush, slot: &str| {
        if path.is_empty() {
            return;
        }
        let clip = framing_of(doc, node, res, id, brush).and_then(|f| f.clip);
        with_image_clip(out, clip, id, slot, &inner, |out, pad| {
            let _ = writeln!(
                out,
                r#"{pad}<path d="{d}"{fill}/>"#,
                d = path_d(path),
                fill = paint_attr("fill", brush, id, slot, defs),
            );
        });
    };
    for run in &layout.runs {
        let glyphs = mastik_core::text::run_outline(run);
        match run.color {
            Some(c) => fill_path(out, &glyphs, &Brush::Solid(c), ""),
            None => {
                for (slot, brush) in &inks {
                    fill_path(out, &glyphs, brush, slot);
                }
            }
        }
    }
    emit_strokes(out, doc, node, res, id, &inner, &strokes, defs);
    for ink in &layout.decorations {
        let band = mastik_render::scene::decoration_path(ink);
        let band = match &layout.warp {
            Some(warp) => warp.warp(&band),
            None => band,
        };
        // Its own colour, else the first ink only, as the canvas draws it.
        match ink.color {
            Some(c) => fill_path(out, &band, &Brush::Solid(c), ""),
            None => {
                let (slot, brush) = inks.first().map_or(("", &black), |(s, b)| (s.as_str(), *b));
                fill_path(out, &band, brush, slot);
            }
        }
    }
    let _ = writeln!(out, "{pad}</g>");
}

/// The pieces a fitted dash pattern is laid along, or `None` when one pattern
/// says it all (`geometry::dash_fit_pieces`).
fn dash_pieces(
    shape: Option<&BezPath>,
    s: &Stroke,
    width: f64,
) -> Option<Vec<(BezPath, Vec<f64>)>> {
    geometry::dash_fit_pieces(s, width, shape?, Dots::KeepRound)
}

/// The path `s` is laid along when it is not `shape` itself (a thick stroke on a
/// spiral starts further out, and a head cuts its end back:
/// `geometry::stroked_path`); such a stroke is its own `<path>`, apart from the
/// fills' element.
fn trimmed_stroke(
    kind: &NodeKind,
    shape: Option<&BezPath>,
    s: &Stroke,
    markers: mastik_core::Markers,
) -> Option<BezPath> {
    match geometry::stroked_path(kind, shape?, s.width, markers) {
        std::borrow::Cow::Owned(path) => Some(path),
        std::borrow::Cow::Borrowed(_) => None,
    }
}

/// A spiral's path for a writer that lays every stroke along one path (Penpot,
/// Sketch): cut where the curve opens to half the stroke's width, as render and
/// SVG lay it, when one stroke and no shown fill paint it. `true` beside it when a
/// cut was needed and could not be taken (several strokes, or a fill the cut would
/// change), for the writer to report. Any other kind's path comes back as it was.
pub(crate) fn spiral_for_one_path(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    kind: &NodeKind,
    path: BezPath,
) -> (BezPath, bool) {
    if !matches!(kind, NodeKind::Spiral { .. }) {
        return (path, false);
    }
    let Some(paint) = mastik_core::paint_box::node_paint(doc, res, id) else {
        return (path, false);
    };
    let cut = |s: &Stroke| match geometry::stroked_path(kind, &path, s.width, Markers::NONE) {
        std::borrow::Cow::Owned(p) => Some(p),
        std::borrow::Cow::Borrowed(_) => None,
    };
    let filled = paint.fills.iter().any(|f| f.visible);
    match paint.strokes.as_slice() {
        [s] if !filled => match cut(s) {
            Some(p) => (p, false),
            None => (path, false),
        },
        strokes => {
            let needed = strokes.iter().any(|s| cut(s).is_some());
            (path, needed)
        }
    }
}

/// The outline the *paints* are built against: a boolean's derived result, a text
/// node's glyph outlines, or the one the kind describes.
///
/// Every consumer of a shape's own path goes through here, since
/// `geometry::local_path(kind)` alone answers `None` for a boolean and a text node.
fn outline(node: &Node, res: &Resolved, id: NodeId) -> Option<BezPath> {
    let kind = res.used_kind_of(node);
    match kind {
        // Glyph outlines from the cached layout, the path the scene walk strokes (§6.3).
        NodeKind::Text { .. } => res.text_layout(id).map(mastik_core::text::outline),
        _ => match res.boolean_path(id) {
            Some(path) => Some(path.clone()),
            None => geometry::local_path(kind),
        },
    }
}

/// The dash pattern for a stroke drawn on `shape`, at `width`.
///
/// Wrapped so neither emit path forgets the dot substitution and fit-to-corners
/// scaling (§6.3). A round cap's zero-length dash is written as zero: SVG draws
/// its dot, and the file then reads back as the same dotted stroke.
fn resolved_dashes(shape: Option<&BezPath>, s: &Stroke, width: f64) -> Vec<f64> {
    match shape {
        Some(path) => geometry::resolved_dashes_for(s, width, path, Dots::KeepRound),
        // No outline to fit; the dot substitution needs none.
        None => geometry::resolved_dashes_for(
            s,
            width,
            &mastik_core::kurbo::BezPath::new(),
            Dots::KeepRound,
        ),
    }
}

/// A shape whose paints do not fit one element: a group carrying the node's
/// transform and opacity, then every visible fill, then every visible stroke.
///
/// Fills first, then strokes, in list order. The transform and opacity sit on the
/// group: one user space for a `clipPath`, and fading paints separately would let
/// the lower ones show through.
#[allow(clippy::too_many_arguments)]
fn emit_stacked(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    transform: Affine,
    opacity: f32,
    pad: &str,
    fills: &[(usize, &Fill)],
    strokes: &[(usize, &Stroke)],
    defs: &Defs,
) {
    let inner = format!("{pad}  ");
    let derived = res.boolean_path(id);
    // The stacked `<g>` is the node, so the name goes here and not on its children.
    let _ = writeln!(
        out,
        "{pad}<g{}{}{}>",
        name_attr(node),
        transform_attr(transform),
        opacity_attr(opacity)
    );
    for (i, f) in fills {
        let slot = format!("fill-{i}");
        if missing_picture(doc, f.brush.drawn()) {
            emit_missing(out, node, res, id, derived, &slot, &inner, defs);
            continue;
        }
        let clip = framing_of(doc, node, res, id, f.brush.drawn()).and_then(|f| f.clip);
        // Per fill, so one contained picture does not cut the fill under it.
        with_image_clip(out, clip, id, &slot, &inner, |out, pad| {
            // Through `with_fill_rule`: a `Boolean` with `BoolOp::Exclude` is
            // even-odd, and without the rule its holes fill in on this path.
            let fill = with_fill_rule(paint_attr("fill", f.brush.drawn(), id, &slot, defs), node);
            write_element(
                out,
                node,
                res.used_kind_of(node),
                derived,
                pad,
                "",
                &fill,
                "",
                "",
                defs,
            );
        });
    }
    emit_strokes(out, doc, node, res, id, &inner, strokes, defs);
    let _ = writeln!(out, "{pad}</g>");
}

/// `fill` with the node's `fill-rule` on it, when that is not SVG's own default.
///
/// Shared by both arms that write a fill. Written only when not `nonzero`, and
/// even when the fill is `none`: a stroke-only shape can still lend its interior to
/// a `clipPath`.
fn with_fill_rule(fill: String, node: &Node) -> String {
    match node.fill_rule() {
        mastik_core::FillRule::NonZero => fill,
        mastik_core::FillRule::EvenOdd => format!(r#"{fill} fill-rule="evenodd""#),
    }
}

/// Every visible stroke of `node`, one after another, inside a group that is already
/// carrying the transform and the opacity.
///
/// Shared by the stacked-shape and frame paths; `write_element` picks the element.
#[allow(clippy::too_many_arguments)]
fn emit_strokes(
    out: &mut String,
    doc: &Document,
    node: &Node,
    res: &Resolved,
    id: NodeId,
    inner: &str,
    strokes: &[(usize, &Stroke)],
    defs: &Defs,
) {
    let shape = outline(node, res, id);
    // Outlined text strokes its glyphs as a `<path>`, as a boolean strokes its result.
    let derived = match node.kind() {
        NodeKind::Text { .. } if defs.text == ExportText::Outlines => shape.as_ref(),
        _ => res.boolean_path(id),
    };
    for (i, s) in strokes {
        // Outlined, the stroke is the ink it lays filled with its paint, which
        // already holds its sides, dashes and alignment, so nothing below applies.
        if defs.style.outline_strokes
            && let Some(ink) = shape
                .as_ref()
                .and_then(|shape| mastik_core::build::stroke_ink(res.used_kind_of(node), shape, s))
        {
            let slot = format!("stroke-{i}");
            let box_clip = framing_of(doc, node, res, id, s.brush.drawn()).and_then(|f| f.clip);
            with_image_clip(out, box_clip, id, &slot, inner, |out, pad| {
                let _ = writeln!(
                    out,
                    r#"{pad}<path d="{d}"{fill}/>"#,
                    d = path_d(&ink),
                    fill = paint_attr("fill", s.brush.drawn(), id, &slot, defs),
                );
            });
            continue;
        }
        // Unfilled, or it would paint over the fills beneath.
        const UNFILLED: &str = r#" fill="none""#;
        // A per-side stroke is one `<path>` per edge (§6.3); the alignment clip still
        // comes from the whole shape and wraps every side together.
        let sides = geometry::effective_sides(res.used_kind_of(node), s).per_side(s.width);
        // A fitted multi-subpath dash is one `<path>` per subpath: an element has
        // one `stroke-dasharray`.
        let trimmed = trimmed_stroke(res.used_kind_of(node), shape.as_ref(), s, s.markers());
        let pieces = dash_pieces(trimmed.as_ref().or(shape.as_ref()), s, s.width);
        let placed = shape.as_ref().map_or_else(Vec::new, |shape| {
            geometry::placed_markers(res.used_kind_of(node), shape, s.width, s.markers())
        });
        // A solid stroke's heads are `<marker>`s; any other paint inside a marker
        // would be laid out against the marker's own box, so those heads are
        // filled paths in the stroke's space instead, as are the heads of a line
        // too short to keep any stroke to hang them on, and of a head whose
        // subpath the trim swallowed: SVG hangs `marker-start` and `marker-end` on
        // the written path's first and last vertex, which is then another
        // subpath's.
        let carrier = |at_end: bool| match &pieces {
            Some(parts) => match at_end {
                true => parts.last(),
                false => parts.first(),
            }
            .map(|(p, _)| p),
            None => trimmed.as_ref().or(shape.as_ref()),
        };
        let hung = placed.iter().all(|p| {
            let vertex = carrier(p.at_end).and_then(|c| match p.at_end {
                true => c
                    .segments()
                    .last()
                    .map(|g| mastik_core::kurbo::ParamCurve::end(&g)),
                false => c
                    .segments()
                    .next()
                    .map(|g| mastik_core::kurbo::ParamCurve::start(&g)),
            });
            vertex.is_some_and(|v| (v - p.base).hypot() <= 1e-6 * p.base.to_vec2().hypot().max(1.0))
        });
        let solid = matches!(s.brush.drawn(), Brush::Solid(_))
            && trimmed.as_ref().is_none_or(|t| !t.is_empty())
            && hung;
        let (mut marker_start, mut marker_end) = (String::new(), String::new());
        if solid {
            for p in &placed {
                let end = if p.at_end { "end" } else { "start" };
                let marker_id = format!("mk-{}-{i}-{end}", id.to_wire().replace(':', "-"));
                emit_marker(out, inner, &marker_id, p, s, id, *i, defs);
                let attr = format!(r#" marker-{end}="url(#{marker_id})""#);
                match p.at_end {
                    true => marker_end = attr,
                    false => marker_start = attr,
                }
            }
        }
        let ink = |out: &mut String, pad: &str, doubled: bool| {
            let scale = if doubled { 2.0 } else { 1.0 };
            let segments = |out: &mut String, parts: &[(BezPath, Vec<f64>)], w: f64| {
                let last = parts.len().saturating_sub(1);
                for (j, (part, dashes)) in parts.iter().enumerate() {
                    let start = if j == 0 { marker_start.as_str() } else { "" };
                    let end = if j == last { marker_end.as_str() } else { "" };
                    let _ = writeln!(
                        out,
                        r#"{pad}<path d="{d}"{UNFILLED}{stroke}{start}{end}/>"#,
                        d = path_d(part),
                        stroke = stroke_attr(id, *i, s, w * scale, dashes, defs),
                    );
                }
            };
            match (&sides, &pieces) {
                (Some(sides), _) => {
                    for (side, w) in sides {
                        let Some(edge) = geometry::side_path(res.used_kind_of(node), *side) else {
                            continue;
                        };
                        // A side is one subpath, so its own fit is exact.
                        let dashes = geometry::resolved_dashes_for(s, *w, &edge, Dots::KeepRound);
                        segments(out, &[(edge, dashes)], *w);
                    }
                }
                (None, Some(parts)) => segments(out, parts, s.width),
                (None, None) if let Some(path) = &trimmed => {
                    let dashes = resolved_dashes(Some(path), s, s.width);
                    segments(out, &[(path.clone(), dashes)], s.width);
                }
                (None, None) => write_element(
                    out,
                    node,
                    res.used_kind_of(node),
                    derived,
                    pad,
                    "",
                    UNFILLED,
                    &format!(
                        "{}{marker_start}{marker_end}",
                        stroke_attr(
                            id,
                            *i,
                            s,
                            s.width * scale,
                            &resolved_dashes(shape.as_ref(), s, s.width),
                            defs,
                        )
                    ),
                    "",
                    defs,
                ),
            }
        };
        // A contained picture letterboxes a stroke as a fill; outside the
        // alignment clip, which cuts a different thing.
        let box_clip = framing_of(doc, node, res, id, s.brush.drawn()).and_then(|f| f.clip);
        with_image_clip(
            out,
            box_clip,
            id,
            &format!("stroke-{i}"),
            inner,
            |out, inner| {
                // Indexed by the paint's own index, stable under hiding.
                let clip_id = format!("sa-{}-{i}", id.to_wire().replace(':', "-"));
                match align_clip(res.used_kind_of(node), shape.as_ref(), s, node.fill_rule()) {
                    Some(AlignCut::Clip(clip_d, rule)) => {
                        let _ = writeln!(
                            out,
                            r#"{inner}<clipPath id="{clip_id}"{rule}><path d="{clip_d}"/></clipPath>"#
                        );
                        let _ = writeln!(out, r#"{inner}<g clip-path="url(#{clip_id})">"#);
                        ink(out, &format!("{inner}  "), true);
                        let _ = writeln!(out, "{inner}</g>");
                    }
                    Some(AlignCut::Mask(ring, shape_d)) => {
                        let (x, y, w, h) = (
                            fmt(ring.x0),
                            fmt(ring.y0),
                            fmt(ring.width()),
                            fmt(ring.height()),
                        );
                        let _ = writeln!(
                            out,
                            r##"{inner}<mask id="{clip_id}" maskUnits="userSpaceOnUse" x="{x}" y="{y}" width="{w}" height="{h}"><rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#fff"/><path d="{shape_d}" fill="#000"/></mask>"##
                        );
                        let _ = writeln!(out, r#"{inner}<g mask="url(#{clip_id})">"#);
                        ink(out, &format!("{inner}  "), true);
                        let _ = writeln!(out, "{inner}</g>");
                    }
                    None => ink(out, inner, false),
                }
                if !solid {
                    for heads in placed.iter().filter_map(|p| p.ink(s.width, s.markers())) {
                        let _ = writeln!(
                            out,
                            r#"{inner}<path d="{d}"{fill}/>"#,
                            d = path_d(&heads),
                            fill = paint_attr(
                                "fill",
                                s.brush.drawn(),
                                id,
                                &format!("stroke-{i}"),
                                defs
                            ),
                        );
                    }
                }
            },
        );
    }
}

/// One head of a solid stroke as a `<marker>` named `marker_id`: drawn in stroke
/// widths, turned to the fixed angle the canvas places it at, and its reference
/// point the stroke's trimmed end, so the tip lands on the outline's end. Its box
/// is the head's own, a width larger all round.
#[allow(clippy::too_many_arguments)]
fn emit_marker(
    out: &mut String,
    pad: &str,
    marker_id: &str,
    p: &mastik_core::marker::Placed,
    s: &Stroke,
    id: NodeId,
    index: usize,
    defs: &Defs,
) {
    let m = s.markers();
    let Some(head) = p.marker.outline(m.size, m.cap, m.join) else {
        return;
    };
    // The trimmed end in the marker's own space, which `refX`/`refY` pin to it.
    let back = (p.base - p.tip) / s.width;
    let along = mastik_core::kurbo::Vec2::new(back.dot(p.dir), p.dir.cross(back));
    let b = head.bounding_box().inflate(1.0, 1.0);
    let (x, y, w, h) = (b.x0, b.y0, b.width(), b.height());
    let _ = writeln!(
        out,
        r#"{pad}<marker id="{marker_id}" markerUnits="strokeWidth" orient="{angle}" refX="{ref_x}" refY="{ref_y}" viewBox="{x} {y} {w} {h}" markerWidth="{w}" markerHeight="{h}" overflow="visible"><path d="{d}"{fill}/></marker>"#,
        angle = fmt(p.dir.atan2().to_degrees()),
        ref_x = fmt(along.x),
        ref_y = fmt(along.y),
        x = fmt(x),
        y = fmt(y),
        w = fmt(w),
        h = fmt(h),
        d = path_d(&head),
        fill = paint_attr(
            "fill",
            s.brush.drawn(),
            id,
            &format!("stroke-{index}"),
            defs
        ),
    );
}

/// A frame's or rect's outline as an element name and its geometry attributes.
/// `<rect rx>` can only say one circular radius, so unequal or smoothed corners
/// go out as the canvas's own outline (`local_path`).
fn box_shape(kind: &NodeKind) -> (&'static str, String) {
    let (NodeKind::Rect {
        size,
        corner_radii,
        corner_smoothing,
    }
    | NodeKind::Artboard {
        size,
        corner_radii,
        corner_smoothing,
    }) = kind
    else {
        return ("path", String::new());
    };
    match geometry::uniform_radius(corner_radii).filter(|r| *r <= 0.0 || *corner_smoothing <= 0.0) {
        Some(r) => {
            // Clamped to half the shorter side, as the outline is: SVG clamps
            // `rx` and `ry` each to its own side, which turns a pill into an
            // ellipse.
            let r = r.min(size.width.min(size.height) * 0.5);
            let rx = if r > 0.0 {
                format!(r#" rx="{}""#, fmt(r))
            } else {
                String::new()
            };
            (
                "rect",
                format!(
                    r#" x="0" y="0" width="{w}" height="{h}"{rx}"#,
                    w = fmt(size.width),
                    h = fmt(size.height),
                ),
            )
        }
        None => {
            let d = geometry::local_path(kind)
                .map(|p| path_d(&p))
                .unwrap_or_default();
            ("path", format!(r#" d="{d}""#))
        }
    }
}

/// The node as one SVG element, with the paint, transform and opacity attributes
/// already built.
///
/// Which element a kind becomes is spelled once; the stacked path calls it per paint.
#[allow(clippy::too_many_arguments)]
fn write_element(
    out: &mut String,
    node: &Node,
    // The node's used kind; every size and outline below is read from it.
    kind: &NodeKind,
    // A path that stands in for the kind's own element: a `Boolean`'s result, or
    // outlined text's glyphs. `None` for every other node.
    derived: Option<&BezPath>,
    pad: &str,
    t: &str,
    fill: &str,
    stroke: &str,
    op: &str,
    // For text: [`Defs::faces`], and the run colours' spelling.
    defs: &Defs,
) {
    match kind {
        NodeKind::Rect { .. } => {
            let (tag, attrs) = box_shape(kind);
            let _ = writeln!(out, r#"{pad}<{tag}{t}{attrs}{fill}{stroke}{op}/>"#);
        }
        // A rounded one has curves a `<polygon>` cannot carry.
        NodeKind::Polygon { corner_radius, .. } | NodeKind::Star { corner_radius, .. }
            if *corner_radius > 0.0 =>
        {
            let d = geometry::local_path(kind)
                .map(|p| path_d(&p))
                .unwrap_or_default();
            let _ = writeln!(out, r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#);
        }
        // `<polygon>` keeps both editable as polygons; the points come from
        // `local_path`.
        NodeKind::Polygon { .. } | NodeKind::Star { .. } => {
            let points = geometry::local_path(kind)
                .map(|p| {
                    p.elements()
                        .iter()
                        .filter_map(|el| match el {
                            mastik_core::kurbo::PathEl::MoveTo(v)
                            | mastik_core::kurbo::PathEl::LineTo(v) => {
                                Some(format!("{},{}", fmt(v.x), fmt(v.y)))
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            let _ = writeln!(
                out,
                r#"{pad}<polygon{t} points="{points}"{fill}{stroke}{op}/>"#
            );
        }
        // An arc has no element of its own, nor has a spiral or an open arc; the
        // outline goes out as a path.
        NodeKind::Ellipse { arc, .. } if !arc.is_whole() => {
            let d = geometry::local_path(kind)
                .map(|p| path_d(&p))
                .unwrap_or_default();
            let _ = writeln!(out, r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#);
        }
        // An unfilled spiral's `derived` is its stroke's cut path (`trimmed_stroke`).
        NodeKind::Spiral { .. } | NodeKind::Arc { .. } => {
            let d = match derived {
                Some(p) => path_d(p),
                None => geometry::local_path(kind)
                    .map(|p| path_d(&p))
                    .unwrap_or_default(),
            };
            let _ = writeln!(out, r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#);
        }
        NodeKind::Ellipse { size, .. } => {
            let _ = writeln!(
                out,
                r#"{pad}<ellipse{t} cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}"{fill}{stroke}{op}/>"#,
                cx = fmt(size.width * 0.5),
                cy = fmt(size.height * 0.5),
                rx = fmt(size.width * 0.5),
                ry = fmt(size.height * 0.5),
            );
        }
        NodeKind::Line { end } => {
            let _ = writeln!(
                out,
                r#"{pad}<line{t} x1="0" y1="0" x2="{x2}" y2="{y2}"{stroke}{op}/>"#,
                x2 = fmt(end.x),
                y2 = fmt(end.y),
            );
        }
        // The resolved outline, so a rounded corner exports rounded: SVG has no
        // per-vertex radius and `path` alone is the unrounded authored geometry.
        NodeKind::Path { .. } => {
            if let Some(resolved) = geometry::local_path(kind) {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(&resolved),
                );
            }
        }
        // A boolean exports as its result, not its operands: SVG has no boolean
        // container, and the operands would show the shapes it consumes.
        NodeKind::Boolean { .. } => {
            if let Some(path) = derived {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(path),
                );
            }
        }
        // One `<text>` per line, one `<tspan>` per run, from `text::export_lines`
        // (the cached `TextLayout` holds glyph ids that do not map back to
        // characters). Ink stays on the `<text>`, the face moves to the `<tspan>`s.
        //
        // A rail is the one place placement goes back to the viewer: a `<text>` per
        // glyph is unreachable (ids do not survive ligatures and reordering) and
        // outlines would lose selectable text. Alignment survives as a
        // `startOffset` percentage.
        NodeKind::Text { on_path, .. } => {
            if let Some(path) = derived {
                let _ = writeln!(
                    out,
                    r#"{pad}<path{t} d="{d}"{fill}{stroke}{op}/>"#,
                    d = path_d(path),
                );
                return;
            }
            let Some(parts) = mastik_core::TextRef::of(kind) else {
                return;
            };
            if on_path.is_some() {
                let mut spans = String::new();
                for line in mastik_core::text::export_lines(parts) {
                    spans.push_str(&tspans(&line.runs, defs));
                }
                // Percent plus `text-anchor` survives a different font. The node's
                // offset is added to the percentage and leaves the anchor alone: the
                // anchor is how text sits about a point, the offset which point.
                let anchored = match parts.paragraph.align {
                    mastik_core::TextAlign::Center => (50.0, " text-anchor=\"middle\""),
                    mastik_core::TextAlign::End | mastik_core::TextAlign::Right => {
                        (100.0, " text-anchor=\"end\"")
                    }
                    _ => (0.0, ""),
                };
                let anchor = anchored.1;
                let offset = format!("{}%", fmt_unit(anchored.0 + parts.on_path_offset * 100.0));
                let _ = writeln!(
                    out,
                    // `r##`, because `href="#` contains the `"#` that would close
                    // an `r#` string on the spot.
                    r##"{pad}<text{t} xml:space="preserve"{fill}{stroke}{op}{anchor}><textPath href="#{rail}" startOffset="{offset}">{spans}</textPath></text>"##,
                    // The same id `collect_defs` named the rail with.
                    rail = rail_id(node.id()),
                );
                return;
            }
            for line in mastik_core::text::export_lines(parts) {
                // No `text-anchor`: alignment, indent and justification are already
                // in the run's `x`, read after `Layout::align`.
                let spans = tspans(&line.runs, defs);
                // `xml:space`: a viewer collapses runs of spaces and drops an
                // indent by default, and the PDF converter is such a viewer.
                let _ = writeln!(
                    out,
                    r#"{pad}<text{t} x="{x}" y="{y}" xml:space="preserve"{fill}{stroke}{op}>{spans}</text>"#,
                    x = fmt(line.x),
                    y = fmt(line.baseline),
                );
            }
        }
        // Only a frame's stroke asks for this; `emit_node` writes the background.
        NodeKind::Artboard { .. } => {
            let (tag, attrs) = box_shape(kind);
            let _ = writeln!(out, r#"{pad}<{tag}{t}{attrs}{fill}{stroke}{op}/>"#);
        }
        // `emit_node` never reaches here with a slice.
        NodeKind::Root | NodeKind::Group | NodeKind::Slice { .. } => {}
    }
}

/// The family name the PDF's font database files a font file under besides its
/// own (`pdf::pdf_framed`), so a run naming it finds the very file that drew it.
pub(crate) fn face_alias((blob, index): mastik_core::text::FaceFile) -> String {
    format!("mastik-face-{blob}-{index}")
}

/// One line's runs as `<tspan>`s. Naming faces ([`Defs::faces`]), a run is cut
/// wherever its face or file changes and names its file's [`face_alias`] ahead
/// of its family; otherwise neighbouring runs of one style are one `<tspan>`, so
/// a face change alone writes nothing.
fn tspans(runs: &[mastik_core::text::ExportRun], defs: &Defs) -> String {
    type Face<'a> = (Option<&'a str>, Option<mastik_core::text::FaceFile>);
    let mut merged: Vec<(String, &mastik_core::TextStyle, Face)> = Vec::new();
    for run in runs {
        let face = if defs.faces {
            (run.face.as_deref(), run.file)
        } else {
            (None, None)
        };
        match merged.last_mut() {
            Some((text, style, f)) if **style == run.style && *f == face => {
                text.push_str(&run.text);
            }
            _ => merged.push((run.text.clone(), &run.style, face)),
        }
    }
    let mut out = String::new();
    for (text, style, (face, file)) in merged {
        let _ = write!(
            out,
            "<tspan{attrs}>{text}</tspan>",
            attrs = run_attrs(style, face, file, defs),
            text = escape_xml(&text),
        );
    }
    out
}

/// Everything one run's style contributes to its `<tspan>`: face, spacing and
/// decorations, the family named `face` when one is given ([`Defs::faces`]),
/// after `file`'s [`face_alias`] when one is given.
/// `agreed` resolves an underline disagreeing with a strikethrough on the same run.
fn run_attrs(
    style: &mastik_core::TextStyle,
    face: Option<&str>,
    file: Option<mastik_core::text::FaceFile>,
    defs: &Defs,
) -> String {
    let mut out = String::new();
    let family = face.unwrap_or(&style.font_family);
    let family = match file {
        Some(file) => format!("{}, {family}", face_alias(file)),
        None => family.to_owned(),
    };
    let _ = write!(
        out,
        r#" font-family="{family}" font-size="{size}" font-weight="{weight}""#,
        family = escape_xml(&family),
        size = fmt(style.font_size),
        weight = style.weight,
    );
    if style.italic {
        out.push_str(r#" font-style="italic""#);
    }
    // Axes and features have no presentation attribute, so they ride CSS in `style`,
    // spelled as the code export spells them. A tag is any four printable bytes, so
    // it is quoted as a CSS string; the XML escape then leaves `'` raw.
    let tag = |t: &mastik_core::Tag| {
        format!(
            "'{}'",
            t.as_str().replace('\\', "\\\\").replace('\'', "\\'")
        )
    };
    let mut css = Vec::new();
    if !style.variations.is_empty() {
        let axes: Vec<String> = style
            .variations
            .iter()
            .map(|a| format!("{} {}", tag(&a.tag), fmt(a.value)))
            .collect();
        css.push(format!("font-variation-settings:{}", axes.join(", ")));
    }
    if !style.features.is_empty() {
        let tags: Vec<String> = style
            .features
            .iter()
            .map(|f| format!("{} {}", tag(&f.tag), f.value))
            .collect();
        css.push(format!("font-feature-settings:{}", tags.join(", ")));
    }
    if !css.is_empty() {
        let css = css.join(";");
        let attr = escape_xml(&css).replace("&apos;", "'");
        let _ = write!(out, r#" style="{attr}""#);
    }
    // An attribute, not a `dy`: a positioned `<tspan>` would read back as a node of
    // its own. Positive raises, as the model's does.
    let shift = style.baseline_shift.resolve(style.font_size);
    if shift != 0.0 {
        let _ = write!(out, r#" baseline-shift="{}""#, fmt(shift));
    }
    if let Some(lang) = &style.locale {
        let _ = write!(out, r#" xml:lang="{}""#, escape_xml(lang));
    }
    // A run's own ink overrides the `<text>`'s; `None` inherits. Alpha rides
    // `fill-opacity`.
    if let Some(c) = style.color {
        out.push_str(&solid_attr("fill", c, defs));
    }
    if !style.letter_spacing.is_zero() {
        let _ = write!(
            out,
            r#" letter-spacing="{}""#,
            fmt(style.letter_spacing.resolve(style.font_size))
        );
    }
    if !style.word_spacing.is_zero() {
        let _ = write!(
            out,
            r#" word-spacing="{}""#,
            fmt(style.word_spacing.resolve(style.font_size))
        );
    }
    let names: Vec<&str> = [
        style.underline.map(|_| "underline"),
        style.strikethrough.map(|_| "line-through"),
    ]
    .into_iter()
    .flatten()
    .collect();
    if names.is_empty() {
        return out;
    }
    let each: Vec<Decoration> = [style.underline, style.strikethrough]
        .into_iter()
        .flatten()
        .collect();
    let _ = write!(out, r#" text-decoration="{}""#, names.join(" "));
    // The three refinements come after the shorthand, which resets its omitted
    // parts. Each is written only when every decoration on the run agrees (CSS
    // gives an element one colour, thickness and style). They are presentation
    // attributes, so a reader ignoring the family ignores all three.
    if let Some(c) = agreed(&each, |d| d.color).flatten() {
        // No `text-decoration-color-opacity` exists, so alpha rides in CSS Color
        // 4's eight-digit hex, only when not opaque.
        let rgba = c.to_rgba8();
        let hex = if rgba.a == 255 {
            format!("#{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b)
        } else {
            format!("#{:02x}{:02x}{:02x}{:02x}", rgba.r, rgba.g, rgba.b, rgba.a)
        };
        let _ = write!(out, r#" text-decoration-color="{hex}""#);
    }
    if let Some(t) = agreed(&each, |d| d.thickness).flatten() {
        // `px`: CSS's unitless length is invalid, unlike `letter-spacing`'s.
        // Floored as the canvas floors it (`core::text::decoration_ink`); the
        // model's clamp is not reached from the op layer, so this is load-bearing:
        // removing it fails `a_decorations_colour_thickness_and_style_are_exported`.
        let _ = write!(
            out,
            r#" text-decoration-thickness="{}px""#,
            fmt(t.resolve(style.font_size).max(0.0))
        );
    }
    // Solid is CSS's initial value as well as ours; writing it is noise.
    if let Some(s) = agreed(&each, |d| d.style).filter(|s| *s != LineStyle::Solid) {
        let _ = write!(out, r#" text-decoration-style="{}""#, line_style_css(s));
    }
    // Skip-ink, only when switched off: CSS's initial `auto` is what the canvas does.
    // Read off the underline alone (it never applies to a `line-through`). `auto`
    // agrees in kind, not pixels; exactness would mean a `<path>` from
    // `DecorationInk::gaps`, at the cost of the `text-decoration-*` family.
    if style.underline.is_some_and(|d| !d.skip_ink) {
        let _ = write!(out, r#" text-decoration-skip-ink="none""#);
    }
    // `text-underline-offset` is underline-only, so a run with both decorations
    // drops it. The sign flips: ours is positive up from the baseline, CSS's
    // positive outward, so a stored `+2` exports as `-2px`. It is not a
    // sub-property of `text-decoration`, so order does not matter.
    if style.strikethrough.is_none()
        && let Some(offset) = style.underline.and_then(|d| d.offset)
    {
        let _ = write!(
            out,
            r#" text-underline-offset="{}px""#,
            fmt(-offset.resolve(style.font_size))
        );
    }
    out
}

/// The value every decoration on one element agrees on, or `None` when they
/// disagree or there are none; `text-decoration-*` is element-scoped in CSS.
fn agreed<T: PartialEq>(ds: &[Decoration], f: impl Fn(&Decoration) -> T) -> Option<T> {
    let mut values = ds.iter().map(f);
    let first = values.next()?;
    values.all(|v| v == first).then_some(first)
}

/// The CSS keyword for a line style; all four of ours are CSS keywords verbatim.
fn line_style_css(s: LineStyle) -> &'static str {
    match s {
        LineStyle::Solid => "solid",
        LineStyle::Dashed => "dashed",
        LineStyle::Dotted => "dotted",
        LineStyle::Wavy => "wavy",
    }
}

/// ` transform="matrix(..)"`, omitted entirely when it is the identity.
fn transform_attr(a: Affine) -> String {
    matrix_attr("transform", a)
}

/// ` data-name="…"` — the layer's name, on the element that is the layer. Not
/// `id`, which must be a unique XML `Name` and would share a namespace with the
/// synthesised ids; `svg_in` prefers it and falls back to `id`. Empty writes nothing.
fn name_attr(node: &Node) -> String {
    match node.name() {
        "" => String::new(),
        name => format!(r#" data-name="{}""#, escape_xml(name)),
    }
}

/// ` <name>="matrix(..)"` for `transform` or `patternTransform`, omitted when it
/// says nothing. Identity is judged on the printed form: a rounded rect's box top
/// is `-8.88e-16` (see [`fmt`]), a hair from identity yet printing as one.
fn matrix_attr(name: &str, a: Affine) -> String {
    let [m0, m1, m2, m3, m4, m5] = fmt_matrix(a);
    if [&m0, &m1, &m2, &m3, &m4, &m5] == ["1", "0", "0", "1", "0", "0"] {
        return String::new();
    }
    format!(r#" {name}="matrix({m0} {m1} {m2} {m3} {m4} {m5})""#)
}

/// How an aligned stroke is cut down to its side of the outline.
enum AlignCut {
    /// A `<clipPath>`: its path data and its ` clip-rule` attribute.
    Clip(String, &'static str),
    /// A `<mask>` over `region`, white but where the shape's path data fills.
    Mask(Rect, String),
}

/// What an aligned stroke has to be cut with, or `None` when the stroke needs no
/// construction. `rule` is the node's fill rule.
///
/// SVG has no stroke alignment, so as on the canvas (§6.3) the stroke is doubled
/// and the wrong half cut away: inside clips to the shape, outside to everything
/// but it (shape plus enclosing box, even-odd). A non-zero path or glyph outline
/// may cross itself, where that even-odd complement counts the overlap as outside,
/// so its outside stroke is masked by the shape's own fill instead, as
/// `scene::paint_strokes` erases it. `None` for a centred stroke, an open shape,
/// or no outline.
fn align_clip(
    kind: &NodeKind,
    shape: Option<&BezPath>,
    s: &Stroke,
    rule: mastik_core::FillRule,
) -> Option<AlignCut> {
    if s.align == StrokeAlign::Center || !geometry::stroke_align_applies(kind) {
        return None;
    }
    let path = shape?;
    Some(match s.align {
        StrokeAlign::Outside => {
            let b = path.bounding_box();
            // The ring must clear the ink, not the shape (as `scene::outer_bounds`):
            // the doubled stroke reaches `limit` times further at a mitre.
            let reach =
                geometry::effective_sides(kind, s).max_width(s.width) * s.miter_limit.max(1.0);
            let pad_out = (b.width() + b.height()).max(1.0) + reach;
            let ring = b.inflate(pad_out, pad_out);
            if rule == mastik_core::FillRule::NonZero
                && matches!(kind, NodeKind::Path { .. } | NodeKind::Text { .. })
            {
                return Some(AlignCut::Mask(ring, path_d(path)));
            }
            let mut complement = ring.to_path(0.1);
            complement.extend(path.iter());
            AlignCut::Clip(path_d(&complement), r#" clip-rule="evenodd""#)
        }
        _ => AlignCut::Clip(
            path_d(path),
            match rule {
                mastik_core::FillRule::NonZero => "",
                mastik_core::FillRule::EvenOdd => r#" clip-rule="evenodd""#,
            },
        ),
    })
}

/// Stroke attributes for one stroke at `width` (a `Custom` side's own, doubled in
/// the aligned construction). `dashes` is already resolved
/// ([`geometry::resolved_dashes`]): SVG has no zero-length dot or "fit to corners".
fn stroke_attr(
    id: NodeId,
    index: usize,
    s: &Stroke,
    width: f64,
    dashes: &[f64],
    defs: &Defs,
) -> String {
    let paint = paint_attr(
        "stroke",
        s.brush.drawn(),
        id,
        &format!("stroke-{index}"),
        defs,
    );
    let join = match s.join {
        mastik_core::kurbo::Join::Miter => "miter",
        mastik_core::kurbo::Join::Round => "round",
        mastik_core::kurbo::Join::Bevel => "bevel",
    };
    let cap = match s.cap {
        mastik_core::kurbo::Cap::Butt => "butt",
        mastik_core::kurbo::Cap::Round => "round",
        mastik_core::kurbo::Cap::Square => "square",
    };
    let dash = if dashes.is_empty() {
        String::new()
    } else {
        let list = dashes.iter().map(|d| fmt(*d)).collect::<Vec<_>>().join(",");
        format!(
            r#" stroke-dasharray="{list}" stroke-dashoffset="{}""#,
            fmt(s.dash_offset)
        )
    };
    // Only when not the default, which SVG puts at 4, as kurbo does.
    let miter = if s.join == mastik_core::kurbo::Join::Miter && s.miter_limit != 4.0 {
        format!(r#" stroke-miterlimit="{}""#, fmt_unit(s.miter_limit))
    } else {
        String::new()
    };
    format!(
        r#"{paint} stroke-width="{w}" stroke-linejoin="{join}" stroke-linecap="{cap}"{dash}{miter}"#,
        w = fmt(width),
    )
}

/// A paint attribute (`fill` or `stroke`): a hex colour for solids, a `url(#…)`
/// reference for gradients and for images.
///
/// `defs` is asked only for an image: `url(#…)` for a paint server not in the file
/// is an SVG error where "no paint" is a picture.
fn paint_attr(attr: &str, brush: &Brush, id: NodeId, slot: &str, defs: &Defs) -> String {
    match brush {
        Brush::Solid(c) => solid_attr(attr, *c, defs),
        // `{attr}-opacity` multiplies the paint server's own alpha, so stops keep
        // `stop-opacity`; through `alpha_attr` so opacity rounds the same everywhere.
        Brush::Gradient(g) => format!(
            r#" {attr}="url(#{})"{}"#,
            gradient_id(id, slot),
            alpha_attr(
                &format!("{attr}-opacity"),
                (g.opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
            ),
        ),
        // A `<pattern>` (see [`image_def`]), else `none`. Only a stroke reaches
        // `none`: a fill gets the placeholder, a stroke has no area for one. A
        // ` none` fallback after the `url(…)` would fix nothing.
        Brush::Image(_) => match defs.has(&image_id(id, slot)) {
            true => format!(r#" {attr}="url(#{})""#, image_id(id, slot)),
            false => format!(r#" {attr}="none""#),
        },
    }
}

/// A solid paint attribute: a hex colour, or `currentColor` when the pass asks
/// for it ([`Style::current_color`]). Either way the alpha rides
/// `{attr}-opacity`, so a duotone of one colour at two opacities survives.
fn solid_attr(attr: &str, c: Color, defs: &Defs) -> String {
    let rgba = c.to_rgba8();
    let alpha = alpha_attr(&format!("{attr}-opacity"), rgba.a);
    if defs.style.current_color {
        defs.merged.borrow_mut().insert([rgba.r, rgba.g, rgba.b]);
        return format!(r#" {attr}="currentColor"{alpha}"#);
    }
    format!(
        r##" {attr}="#{:02x}{:02x}{:02x}"{alpha}"##,
        rgba.r, rgba.g, rgba.b,
    )
}

fn alpha_attr(name: &str, alpha: u8) -> String {
    if alpha == 255 {
        String::new()
    } else {
        format!(r#" {name}="{}""#, fmt_unit(alpha as f64 / 255.0))
    }
}

/// The style of the `<g>` a node's blend mode wraps it in, or `None` when it
/// draws straight into its parent.
fn blend_style(doc: &Document, node: &mastik_core::Node) -> Option<String> {
    blend_decl(doc, node).map(|(prop, value)| format!("{prop}:{value}"))
}

/// The CSS declaration a node's blend mode needs, or `None` when it draws
/// straight into its parent — `scene::blend_layer`'s decision, made again for
/// the SVG writer and code export. A Normal that isolates is `isolation:
/// isolate`; a top-level frame holding a blended layer takes one so what is
/// inside blends with the frame alone.
pub(crate) fn blend_decl(
    doc: &Document,
    node: &mastik_core::Node,
) -> Option<(&'static str, &'static str)> {
    let blend = node.blend();
    if blend.isolates(node.kind()) {
        return Some(match blend {
            BlendMode::Normal => ("isolation", "isolate"),
            mode => ("mix-blend-mode", mode.css()),
        });
    }
    let page = matches!(node.kind(), NodeKind::Root) && node.parent().is_none();
    let top_frame = matches!(node.kind(), NodeKind::Artboard { .. })
        && node
            .parent()
            .and_then(|p| doc.get(p))
            .is_some_and(|p| matches!(p.kind(), NodeKind::Root));
    ((page || top_frame) && holds_blend(doc, node)).then_some(("isolation", "isolate"))
}

/// Whether a layer under `node` blends with what is beneath it — its mode or a
/// visible shadow's, as `Resolved::blends_within` reads it for the canvas.
pub(crate) fn holds_blend(doc: &Document, node: &mastik_core::Node) -> bool {
    node.children().iter().filter_map(|c| doc.get(*c)).any(|c| {
        let shadow = c.effects().iter().any(|e| match &e.kind {
            EffectKind::DropShadow(s) | EffectKind::InnerShadow(s) => e.visible && s.blends(),
            _ => false,
        });
        !matches!(c.blend(), BlendMode::Normal | BlendMode::PassThrough)
            || shadow
            || holds_blend(doc, c)
    })
}

fn opacity_attr(opacity: f32) -> String {
    if opacity >= 1.0 {
        String::new()
    } else {
        format!(r#" opacity="{}""#, fmt_unit(opacity as f64))
    }
}

/// A `d` attribute, rounded like every other number (`BezPath::to_svg` prints
/// full `f64` precision). The JSON snapshot's path is this string too: full
/// precision carried the platform's math library into the 16th digit.
pub(crate) fn path_d(path: &mastik_core::kurbo::BezPath) -> String {
    use mastik_core::kurbo::PathEl;
    let p = |pt: mastik_core::kurbo::Point| format!("{},{}", fmt(pt.x), fmt(pt.y));
    let mut out = String::new();
    for el in path.elements() {
        if !out.is_empty() {
            out.push(' ');
        }
        match el {
            PathEl::MoveTo(a) => out.push_str(&format!("M{}", p(*a))),
            PathEl::LineTo(a) => out.push_str(&format!("L{}", p(*a))),
            PathEl::QuadTo(a, b) => out.push_str(&format!("Q{} {}", p(*a), p(*b))),
            PathEl::CurveTo(a, b, c) => {
                out.push_str(&format!("C{} {} {}", p(*a), p(*b), p(*c)));
            }
            PathEl::ClosePath => out.push('Z'),
        }
    }
    out
}

/// Format a float compactly: integers without a decimal point, else trimmed.
/// Shared with the code export, whose CSS numbers want the same spelling.
pub(crate) fn fmt(v: f64) -> String {
    // A non-finite number is written as `0`: `NaN`/`inf` is no SVG `<number>` and
    // would make the whole file unparseable. A floor; producers are guarded at their
    // own ends (e.g. [`effect_region`]).
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    fmt_places(v, PRECISION.with(|p| p.get()) as usize)
}

/// [`fmt`] at [`MAX_PRECISION`](mastik_core::export::MAX_PRECISION) whatever the
/// spec's precision: for unitless numbers (opacities, stop offsets, colour
/// matrices, a miter limit, a text path's offset), where a rounding error scales
/// with what they multiply.
fn fmt_unit(v: f64) -> String {
    fmt_fixed(v, mastik_core::export::MAX_PRECISION as usize)
}

/// Significant digits [`fmt_matrix`] keeps of a matrix's largest scale term.
const MATRIX_DIGITS: i32 = 7;

/// An affine's six coefficients as [`fmt_unit`] writes them, but with at least
/// [`MATRIX_DIGITS`] significant digits of its largest scale term: a large
/// picture's pattern scale is a small number multiplying thousands of pixels,
/// and four places leave its far edge a visible fraction of a unit short.
fn fmt_matrix(a: Affine) -> [String; 6] {
    let coeffs = a.as_coeffs();
    let scale = coeffs[..4].iter().fold(0.0f64, |m, v| m.max(v.abs()));
    let floor = mastik_core::export::MAX_PRECISION as i32;
    let places = match scale.is_finite() && scale > 0.0 {
        // Capped so a degenerate scale cannot ask for hundreds of places.
        true => (MATRIX_DIGITS - 1 - scale.log10().floor() as i32).clamp(floor, 17),
        false => floor,
    };
    coeffs.map(|v| fmt_fixed(v, places as usize))
}

/// `v` at `places` decimals, whole numbers bare and a non-finite number `0`.
fn fmt_fixed(v: f64, places: usize) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if v.fract() == 0.0 && v.abs() < 1e15 {
        return format!("{}", v as i64);
    }
    fmt_places(v, places)
}

/// `v` at `places` decimals, trailing zeros trimmed only after a decimal point:
/// at 0 places there is none, and trimming would eat the integer's own zeros.
fn fmt_places(v: f64, places: usize) -> String {
    let s = format!("{v:.places$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        &s
    };
    // `-0` is dust (a rounded rect's box top is `-8.88e-16`), never worth writing.
    match s {
        "-0" => "0".to_string(),
        s => s.to_string(),
    }
}

/// The credits of the layers under `origins` (`mastik_core::credit`), as one
/// `<metadata>` block at the top of the file: one `<credit>` per folded line of
/// `credits_in`, every credit whether or not its licence asks to be shown, naming
/// its layers. Nothing when no layer carries one. At the
/// top rather than inside each element, since a shape is written as several
/// elements and a wrapper `<g>` would come back as a group.
fn credits_metadata(out: &mut String, doc: &Document, origins: &[NodeId]) {
    let listed = mastik_core::credit::credits_in(doc, origins);
    if listed.is_empty() {
        return;
    }
    out.push_str("  <metadata>\n");
    out.push_str("    <credits xmlns=\"https://github.com/fadion/mastik/credits\">\n");
    for l in &listed {
        let c = &l.credit;
        let layers: Vec<&str> = l
            .layers
            .iter()
            .filter_map(|id| doc.get(*id).map(Node::name))
            .collect();
        let mut attrs = format!(
            r#" layers="{}" licence="{}""#,
            escape_xml(&layers.join(", ")),
            escape_xml(&c.licence),
        );
        for (name, url) in [
            ("source", &c.source_url),
            ("author", &c.author_url),
            ("licence-url", &c.licence_url),
        ] {
            if !url.is_empty() {
                let _ = write!(attrs, r#" {name}="{}""#, escape_xml(url));
            }
        }
        let _ = writeln!(out, "      <credit{attrs}>{}</credit>", escape_xml(&c.text));
    }
    out.push_str("    </credits>\n");
    out.push_str("  </metadata>\n");
}

/// `s` as XML text or attribute content. A char outside XML 1.0's `Char`
/// production (C0 controls but tab, LF and CR; U+FFFE/U+FFFF) is dropped: one
/// would make the whole file unparseable, and credit text and names come from
/// the network and from imports.
pub(crate) fn escape_xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' | '\n' | '\r' => out.push(ch),
            '\u{0}'..='\u{1f}' | '\u{fffe}' | '\u{ffff}' => {}
            _ => out.push(ch),
        }
    }
    out
}

/// Unused now that gradients resolve to defs, but kept as the single place a
/// brush collapses to one colour if a future consumer needs it.
#[allow(dead_code)]
fn brush_color(brush: &Brush) -> Color {
    match brush {
        Brush::Solid(c) => *c,
        _ => Color::from_rgba8(128, 128, 128, 255),
    }
}

#[cfg(test)]
mod credit_tests {
    use super::*;
    use mastik_core::kurbo::Size;
    use mastik_core::{IdSource, Operation, Transaction};

    /// Code export's inline pictures carry no `<credits>` (the page's comment
    /// does), while an export spec's file keeps them.
    /// flip: `credits: true` in `svg_at_zero` → fails at the first assertion.
    #[test]
    fn inline_pictures_leave_the_credits_to_the_page() {
        let mut ids = IdSource::new(0xC4ED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = doc.first_page();
        let id = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetCredit {
                id,
                credit: Some(Box::new(mastik_core::Credit {
                    kind: mastik_core::CreditKind::Icon,
                    text: "Icons by Font Awesome, CC BY 4.0".into(),
                    author: "Font Awesome".into(),
                    author_url: String::new(),
                    provider: "Font Awesome Free".into(),
                    provider_url: String::new(),
                    source_url: String::new(),
                    licence: "CC BY 4.0".into(),
                    licence_url: String::new(),
                    required: true,
                    inserted: String::new(),
                })),
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let inline = svg_at_zero(&doc, &res, &[id], Default::default(), ExportText::Live);
        assert!(!inline.svg.contains("<credits"), "{}", inline.svg);
        let spec = ExportSpec::new(
            mastik_core::ExportFormat::Svg,
            mastik_core::ExportScale::Times(1.0),
        );
        assert!(svg_spec(&doc, &res, id, &spec).svg.contains("<credits"));
    }

    /// A control character in network credit text is dropped, so the file parses;
    /// tab, LF and CR are legal XML and stay.
    /// flip: `escape_xml` without its control arm → fails at the parse.
    #[test]
    fn control_characters_never_reach_the_file() {
        assert_eq!(escape_xml("a\u{1}b\u{b}c\u{fffe}"), "abc");
        assert_eq!(escape_xml("a\tb\nc\r"), "a\tb\nc\r");
        let xml = format!("<credit>{}</credit>", escape_xml("“Sunset\u{1b}” by Ana"));
        assert!(roxmltree::Document::parse(&xml).is_ok(), "{xml}");
    }
}

#[cfg(test)]
mod spiral_tests {
    use super::*;
    use mastik_core::kurbo::{PathEl, Size};
    use mastik_core::{IdSource, Operation, Transaction, keyed_by_position};

    /// A spiral and its id, stroked `width` wide, filled when `filled`.
    fn spiral(width: f64, filled: bool) -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(0x5917);
        let mut doc = Document::new(ids.mint());
        let page = doc.first_page();
        let id = ids.mint();
        let mut ops = vec![
            Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Spiral {
                    size: Size::new(100.0, 100.0),
                    turns: 3.0,
                },
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id,
                strokes: keyed_by_position([Stroke {
                    brush: Brush::Solid(Color::BLACK).into(),
                    width,
                    ..Default::default()
                }]),
            },
        ];
        if filled {
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([mastik_core::Fill {
                    brush: Brush::Solid(Color::WHITE).into(),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, id)
    }

    fn first_point(p: &BezPath) -> mastik_core::kurbo::Point {
        match p.elements()[0] {
            PathEl::MoveTo(p) => p,
            _ => panic!("a path starts with a move"),
        }
    }

    /// Stroked thicker than its innermost bend, an unfilled spiral is written from
    /// the cut, as render and SVG lay it; filled, it keeps its path and says the
    /// stroke runs from the centre.
    /// flip: `spiral_for_one_path` answering the path unchanged → fails at "cut".
    #[test]
    fn a_thick_spiral_is_written_from_its_cut() {
        let (doc, res, id) = spiral(12.0, false);
        let kind = doc.get(id).unwrap().kind().clone();
        let whole = res.local_path(&doc, id).unwrap();
        let (path, spike) = spiral_for_one_path(&doc, &res, id, &kind, whole.clone());
        assert!(!spike);
        assert_ne!(first_point(&path), first_point(&whole), "cut");

        let (doc, res, id) = spiral(12.0, true);
        let whole = res.local_path(&doc, id).unwrap();
        let (path, spike) = spiral_for_one_path(&doc, &res, id, &kind, whole.clone());
        assert!(spike, "reported");
        assert_eq!(first_point(&path), first_point(&whole));
    }
}

#[cfg(test)]
mod fmt_tests {
    use super::*;

    /// At no places a number rounds to its whole number with its zeros kept, and
    /// a fraction under one half is `0`, never empty.
    /// flip: `fmt_places` trimming without a decimal point → fails at `19.7`.
    #[test]
    fn no_places_keeps_whole_numbers_whole() {
        let _places = PrecisionScope::set(0);
        assert_eq!(fmt(19.7), "20");
        assert_eq!(fmt(100.3), "100");
        assert_eq!(fmt(0.4), "0");
        assert_eq!(fmt(-0.4), "0");
        assert_eq!(fmt(9.6), "10");
        assert_eq!(fmt_unit(0.4), "0.4");
    }

    /// At two places the trim still stops at the point.
    #[test]
    fn two_places_trims_trailing_zeros() {
        let _places = PrecisionScope::set(2);
        assert_eq!(fmt(25.333_333), "25.33");
        assert_eq!(fmt(10.004), "10");
        assert_eq!(fmt(0.5), "0.5");
        assert_eq!(fmt_unit(0.222_222), "0.2222");
    }
}

#[cfg(test)]
mod def_count_tests {
    //! Collecting defs does not compare ids quadratically — counted, not timed.
    use super::*;

    /// `n` rects, each with its own linear gradient fill: `n` defs.
    fn gradients(n: usize) -> (Document, Resolved) {
        use mastik_core::kurbo::{Point, Size};
        use mastik_core::peniko::Gradient;
        use mastik_core::{Fill, IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0xDEF5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut ops = Vec::with_capacity(n * 2);
        for i in 0..n {
            let id = ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            });
            let g =
                Gradient::new_linear(Point::new(0.0, 0.0), Point::new(10.0, 10.0)).with_stops([
                    (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
                    (1.0_f32, Color::from_rgba8((i % 256) as u8, 0, 255, 255)),
                ]);
            ops.push(Operation::SetFills {
                id,
                fills: mastik_core::keyed_by_position([Fill {
                    brush: Brush::Gradient(g.into()).into(),
                    visible: true,
                }]),
            });
        }
        doc.apply(&Transaction(ops)).expect("the fixture");
        let res = Resolved::rebuild(&doc);
        (doc, res)
    }

    /// An export of D defs compares fewer than D ids (a scan compares about D²/2).
    ///
    /// flip: `Defs::add` back to the linear scan → fails at the comparison-count assertion.
    #[test]
    fn collecting_defs_compares_fewer_ids_than_there_are_defs() {
        const N: usize = 2_000;
        let (doc, res) = gradients(N);
        DEF_KEY_COMPARISONS.with(|c| c.set(0));
        let out = svg(&doc, &res, doc.first_page(), None);
        let compared = DEF_KEY_COMPARISONS.with(std::cell::Cell::get);
        assert_eq!(
            out.matches("<linearGradient").count(),
            N,
            "the fixture: every rect must earn a def"
        );
        assert!(
            compared < N as u64,
            "collecting {N} defs compared ids {compared} times — a set compares a \
             handful per insert and a scan compares every id already collected, so \
             a count near N²/2 = {} is the square coming back",
            (N * N / 2)
        );
    }
}

#[cfg(test)]
mod sweep_tests {
    use super::*;
    use mastik_core::GradientBrush;
    use mastik_core::kurbo::Point;
    use mastik_core::peniko::{Gradient, SweepGradientPosition};

    /// A full-turn sweep from `stops`, centred in a 100 × 100 box.
    fn sweep(stops: &[(f32, Color)], extend: Extend) -> String {
        sweep_over(stops, extend, 0.0, std::f32::consts::TAU)
    }

    /// `sweep` from the turn `start` to `end`.
    fn sweep_over(stops: &[(f32, Color)], extend: Extend, start: f32, end: f32) -> String {
        let mut g = Gradient::new_sweep(Point::new(50.0, 50.0), start, end).with_stops(stops);
        g.extend = extend;
        let brush: GradientBrush = g.into();
        let GradientKind::Sweep(p) = brush.gradient.kind else {
            unreachable!()
        };
        sweep_def(
            "g",
            &brush,
            &brush.gradient,
            &SweepGradientPosition { ..p },
            Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
        )
    }

    /// The value of the attribute `name` in the tag `tag` starts.
    fn attr(tag: &str, name: &str) -> String {
        let at = tag.find(&format!(" {name}=\"")).expect("the attribute") + name.len() + 3;
        tag[at..].split('"').next().unwrap().to_owned()
    }

    /// The stop colours of wedge `i`, `[first, last]`, as written.
    fn wedge(svg: &str, i: usize) -> [String; 2] {
        let at = svg
            .find(&format!("id=\"g-w{i}\""))
            .expect("the wedge is written");
        let mut colours = svg[at..].match_indices("stop-color=\"").map(|(k, m)| {
            let s = at + k + m.len();
            svg[s..s + 7].to_owned()
        });
        [colours.next().unwrap(), colours.next().unwrap()]
    }

    /// A red-to-blue sweep starts red, ends blue and passes the canvas's mix half
    /// way round, in wedges no wider than `WEDGE_MAX`, inside one pattern.
    #[test]
    fn a_sweep_is_written_as_wedges_round_its_turn() {
        let svg = sweep(
            &[
                (0.0, Color::from_rgb8(255, 0, 0)),
                (1.0, Color::from_rgb8(0, 0, 255)),
            ],
            Extend::Pad,
        );
        let n = svg.matches("<linearGradient").count();
        assert_eq!(n, 72, "a full turn in 5° wedges");
        // A tile's content counts from its corner; without this the wedges' centre
        // lands at the box's corner plus the centre.
        let tile = svg.split("<pattern").nth(1).expect("a pattern");
        let (x, y) = (attr(tile, "x"), attr(tile, "y"));
        let side = attr(tile, "width");
        assert_eq!(attr(tile, "viewBox"), format!("{x} {y} {side} {side}"));
        assert_eq!(svg.matches("<pattern id=\"g\"").count(), 1);
        assert_eq!(wedge(&svg, 0)[0], "#ff0000");
        assert_eq!(wedge(&svg, n - 1)[1], "#0000ff");
        // Half way the premultiplied mix of the two, which sRGB stops write as #800080.
        assert_eq!(wedge(&svg, n / 2)[0], "#800080");
    }

    /// A hard stop half way round falls on a wedge edge, its two sides in the two
    /// wedges. flip: `INSIDE` at 0 → fails at "the far side".
    #[test]
    fn a_hard_stop_keeps_both_sides() {
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        let svg = sweep(
            &[(0.0, red), (0.5, red), (0.5, blue), (1.0, blue)],
            Extend::Pad,
        );
        let n = svg.matches("<linearGradient").count();
        assert_eq!(n, 72, "the stop's edge adds no sliver");
        let half = n / 2;
        assert_eq!(
            wedge(&svg, half - 1),
            ["#ff0000".to_owned(), "#ff0000".to_owned()]
        );
        assert_eq!(
            wedge(&svg, half),
            ["#0000ff".to_owned(), "#0000ff".to_owned()],
            "the far side"
        );
    }

    /// Each wedge path's turns, its first and last outer vertex: the first in
    /// `-0.05..2π - 0.05`, between an overlap and a wedge, so the first wedge's
    /// reach under the seam reads below 0 and the last wedge's start near `2π`;
    /// the last that far round from the first, so a wedge past the seam reads
    /// past `2π`.
    fn wedge_turns(svg: &str) -> Vec<(f64, f64)> {
        let turn = |x: f64, y: f64| {
            let a = (y - 50.0).atan2(x - 50.0);
            if a < -0.05 {
                a + std::f64::consts::TAU
            } else {
                a
            }
        };
        svg.match_indices("<path d=\"M")
            .map(|(k, m)| {
                let d = svg[k + m.len()..].split('"').next().unwrap();
                let n: Vec<f64> = d
                    .split(['L', ' ', 'Z'])
                    .filter(|s| !s.is_empty())
                    .map(|s| s.parse().unwrap())
                    .collect();
                // The outer two, past the band's corners by the centre.
                let outer: Vec<_> = n
                    .chunks(2)
                    .filter(|p| (p[0] - 50.0).hypot(p[1] - 50.0) > 10.0)
                    .collect();
                let first = turn(outer[0][0], outer[0][1]);
                let round =
                    (turn(outer[1][0], outer[1][1]) - first).rem_euclid(std::f64::consts::TAU);
                (first, first + round)
            })
            .collect()
    }

    /// The first wedge reaches back under the seam and no wedge reaches past it,
    /// even beside a sliver of a last wedge.
    /// flip: `sweep_def` without the `min(TAU)` → fails at "past the seam".
    #[test]
    fn no_wedge_paints_past_the_seam() {
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        let svg = sweep_over(
            &[(0.0, red), (1.0, blue)],
            Extend::Pad,
            0.0,
            std::f32::consts::TAU - 0.003,
        );
        let turns = wedge_turns(&svg);
        assert!(
            turns[0].0 < 0.0,
            "the first reaches under the seam: {:?}",
            turns[0]
        );
        for (i, (_, end)) in turns.iter().enumerate().skip(1) {
            assert!(
                *end <= std::f64::consts::TAU + 1e-6,
                "wedge {i} past the seam: {end}"
            );
        }
    }

    /// A reflected quarter-turn sweep with a hard stop a quarter in puts its edges
    /// at 22.5° and, mirrored on the second quarter, at 157.5°; a repeated one
    /// turned to start at a quarter begins its wedges there.
    /// flip: the reflect edge rule reading `o` on odd turns → fails at "mirrored:
    /// the fourth".
    #[test]
    fn a_reflected_or_turned_sweep_keeps_its_edges_and_start() {
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        let hard = [(0.0, red), (0.25, red), (0.25, blue), (1.0, blue)];
        let quarter = std::f32::consts::FRAC_PI_2;
        let svg = sweep_over(&hard, Extend::Reflect, 0.0, quarter);
        let turns = wedge_turns(&svg);
        // Which wedge holds the turn `a` (in degrees), and its colours.
        let at = |deg: f64| {
            let a = deg.to_radians();
            let i = turns
                .iter()
                .position(|(s, e)| *s <= a && a < *e)
                .expect("a wedge holds it");
            wedge(&svg, i)
        };
        let red_s = ["#ff0000".to_owned(), "#ff0000".to_owned()];
        let blue_s = ["#0000ff".to_owned(), "#0000ff".to_owned()];
        assert_eq!(at(10.0), red_s, "the first");
        assert_eq!(at(50.0), blue_s, "the second");
        assert_eq!(at(130.0), blue_s, "mirrored: the third");
        // Just past the mirrored edge, where a wedge across it would mix the two.
        assert_eq!(at(159.0), red_s, "mirrored: the fourth");
        let turned = sweep_over(&hard, Extend::Repeat, quarter, quarter + quarter);
        // 22.5° between edges, in five wedges of 4.5°.
        let first = wedge_turns(&turned)[1].0;
        assert!(
            (first - (std::f64::consts::FRAC_PI_2 + 4.5f64.to_radians())).abs() < 1e-3,
            "the second wedge starts a wedge past the quarter: {first}"
        );
    }

    /// A turned sweep's wedges are laid out in the gradient's own space and the
    /// pattern carries the brush's transform; an unturned one carries none.
    /// flip: `sweep_def` writing no `patternTransform` → fails at "turned".
    #[test]
    fn a_turned_sweep_carries_its_transform() {
        let (red, blue) = (Color::from_rgb8(255, 0, 0), Color::from_rgb8(0, 0, 255));
        let write = |transform: Affine| {
            let g = Gradient::new_sweep(Point::new(50.0, 50.0), 0.0, std::f32::consts::TAU)
                .with_stops([(0.0, red), (1.0, blue)]);
            let mut brush: GradientBrush = g.into();
            brush.transform = transform;
            let GradientKind::Sweep(p) = brush.gradient.kind else {
                unreachable!()
            };
            sweep_def(
                "g",
                &brush,
                &brush.gradient,
                &p,
                Some(Rect::new(0.0, 0.0, 100.0, 100.0)),
            )
        };
        let plain = write(Affine::IDENTITY);
        assert!(!plain.contains("patternTransform"), "the control: {plain}");
        let turned = write(Affine::rotate_about(
            std::f64::consts::FRAC_PI_2,
            Point::new(50.0, 50.0),
        ));
        let tile = turned.split("<pattern").nth(1).expect("a pattern");
        assert!(
            tile.starts_with(
                r#" id="g" patternUnits="userSpaceOnUse" patternTransform="matrix(0 1 -1 0 100 0)""#
            ),
            "turned: {tile}"
        );
        // The wedges themselves are the unturned ones: the turn is the pattern's.
        assert_eq!(
            wedge_turns(&turned)[1],
            wedge_turns(&plain)[1],
            "laid out in the gradient's space"
        );
    }
}
