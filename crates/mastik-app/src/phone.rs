//! Play on a phone: the app serves the prototype player to the local network
//! (`docs/design/prototyping-player.md`, *Play on a phone*). A [`Share`] binds
//! the computer's private-network address on a free port and answers four
//! paths under a random token, to clients in the private ranges alone: the page
//! (the HTML export's player in mirror mode), the document's current version,
//! a stream of new versions, and each phone's report of its screen. Nothing
//! listens otherwise, and dropping the share stops it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// How often a quiet stream is written to, so a phone that slept is noticed.
const HEARTBEAT: Duration = Duration::from_secs(10);
/// How long a phone that stopped listening stays in the list, greyed, waiting
/// to wake before it is dropped.
const FORGET: Duration = Duration::from_secs(60);
/// The longest request head and body read; anything longer is refused.
const MAX_HEAD: usize = 8 * 1024;
const MAX_BODY: usize = 4 * 1024;
/// How long a connection may take to send its request or take one write.
const IO_WAIT: Duration = Duration::from_secs(5);
/// The most a reply hands the socket in one write, so [`IO_WAIT`] bounds each
/// piece's progress rather than the whole body: Winsock can fail a blocking
/// send that outlasts the timeout whole, rather than return what it sent.
const PIECE: usize = 64 * 1024;

/// A phone following the share.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Phone {
    /// The id its page chose as it loaded.
    id: String,
    /// What its browser says it is, at its coarsest: *Android · Chrome*.
    pub(crate) name: String,
    /// The frame it shows, by name, as it last reported.
    pub(crate) screen: String,
    /// Its stream is open; a phone that sleeps closes it.
    pub(crate) live: bool,
    /// When it was last heard, for [`FORGET`].
    seen: Instant,
}

/// What the share serves, and who follows it.
struct Shared {
    token: String,
    /// The page as it was written, for the version it carries.
    page: Arc<Vec<u8>>,
    /// The document's current version, saved, and its number.
    doc: Arc<Vec<u8>>,
    version: u64,
    phones: Vec<Phone>,
    /// Each phone's open stream of versions, by its id.
    streams: Vec<(String, TcpStream)>,
}

impl Shared {
    /// Write `data` as an event to every stream, closing the ones that fail.
    fn send(&mut self, data: &str) {
        let line = format!("data: {data}\n\n");
        self.write_all(line.as_bytes());
    }

    fn write_all(&mut self, bytes: &[u8]) {
        let mut gone = Vec::new();
        self.streams.retain_mut(|(id, s)| {
            let ok = s.write_all(bytes).and_then(|()| s.flush()).is_ok();
            if !ok {
                gone.push(id.clone());
            }
            ok
        });
        for id in gone {
            self.went_quiet(&id);
        }
    }

    /// The phone `id`, made when first heard from, named by `agent`.
    fn phone(&mut self, id: &str, agent: &str) -> &mut Phone {
        let at = match self.phones.iter().position(|p| p.id == id) {
            Some(at) => at,
            None => {
                self.phones.push(Phone {
                    id: id.to_string(),
                    name: browser_name(agent),
                    screen: String::new(),
                    live: false,
                    seen: Instant::now(),
                });
                self.phones.len() - 1
            }
        };
        let p = &mut self.phones[at];
        p.seen = Instant::now();
        p
    }

    /// `id`'s stream closed: greyed unless another of its streams is open.
    fn went_quiet(&mut self, id: &str) {
        let open = self.streams.iter().any(|(s, _)| s == id);
        if let Some(p) = self.phones.iter_mut().find(|p| p.id == id) {
            p.live = open;
        }
    }
}

/// Why sharing could not start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Refused {
    /// No private-network address: the computer is on no network, or only a
    /// public one, which the share never listens on.
    NoNetwork,
    /// The address would not take a listener; the system's words.
    Bind(String),
}

/// A share running: its address, and the thread answering it.
pub(crate) struct Share {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    /// The page's address, as the QR code and the link give it.
    pub(crate) url: String,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Share {
    /// Start sharing `page` (written for `version`) and `doc` (that version)
    /// on `ip`: this computer's private-network address ([`local_ip`]), or
    /// loopback in a test. `wake` is called from the share's threads when the
    /// phones change, to repaint the card.
    pub(crate) fn start_on(
        ip: IpAddr,
        page: Vec<u8>,
        doc: Vec<u8>,
        version: u64,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Share, Refused> {
        let listener =
            TcpListener::bind(SocketAddr::new(ip, 0)).map_err(|e| Refused::Bind(e.to_string()))?;
        listener
            .set_nonblocking(true)
            .map_err(|e| Refused::Bind(e.to_string()))?;
        let port = listener
            .local_addr()
            .map_err(|e| Refused::Bind(e.to_string()))?
            .port();
        let token = token();
        let url = format!("http://{ip}:{port}/{token}/");
        let shared = Arc::new(Mutex::new(Shared {
            token,
            page: Arc::new(page),
            doc: Arc::new(doc),
            version,
            phones: Vec::new(),
            streams: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let wake: Arc<dyn Fn() + Send + Sync> = Arc::new(wake);
        let thread = {
            let (shared, stop) = (shared.clone(), stop.clone());
            std::thread::Builder::new()
                .name("phone-share".into())
                .spawn(move || serve(listener, shared, stop, wake))
                .map_err(|e| Refused::Bind(e.to_string()))?
        };
        Ok(Share {
            shared,
            stop,
            url,
            thread: Some(thread),
        })
    }

    /// The phones following, in the order they came; a quiet one stays a
    /// minute before it goes.
    pub(crate) fn phones(&self) -> Vec<Phone> {
        let mut s = lock(&self.shared);
        s.phones.retain(|p| p.live || p.seen.elapsed() < FORGET);
        s.phones.clone()
    }

    /// Hand the phones `doc` as version `version`: each fetches it and plays it
    /// where it is.
    pub(crate) fn publish(&self, doc: Vec<u8>, version: u64) {
        let mut s = lock(&self.shared);
        s.doc = Arc::new(doc);
        s.version = version;
        s.send(&version.to_string());
    }

    /// Hand the phones a new page (written for `version`, with `doc`) and have
    /// them load it: what a document needing new fonts takes.
    pub(crate) fn reload(&self, page: Vec<u8>, doc: Vec<u8>, version: u64) {
        let mut s = lock(&self.shared);
        s.page = Arc::new(page);
        s.doc = Arc::new(doc);
        s.version = version;
        s.send("reload");
    }
}

impl Drop for Share {
    /// Stop: the phones are told, so each dims its last frame, and the port
    /// closes.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn lock(m: &Mutex<Shared>) -> MutexGuard<'_, Shared> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// The accept loop: each allowed connection answered on a thread of its own,
/// the streams kept warm, until `stop`.
fn serve(
    listener: TcpListener,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    wake: Arc<dyn Fn() + Send + Sync>,
) {
    let mut beat = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if !allowed(peer.ip()) {
                    continue;
                }
                let (shared, wake) = (shared.clone(), wake.clone());
                let _ = std::thread::Builder::new()
                    .name("phone-share-request".into())
                    .spawn(move || answer(stream, &shared, &*wake));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(_) => std::thread::sleep(Duration::from_millis(50)),
        }
        if beat.elapsed() >= HEARTBEAT {
            beat = Instant::now();
            let mut s = lock(&shared);
            let before: Vec<bool> = s.phones.iter().map(|p| p.live).collect();
            s.write_all(b": beat\n\n");
            let after: Vec<bool> = s.phones.iter().map(|p| p.live).collect();
            drop(s);
            if before != after {
                wake();
            }
        }
    }
    let mut s = lock(&shared);
    s.send("stop");
    for (_, st) in s.streams.drain(..) {
        let _ = st.shutdown(std::net::Shutdown::Both);
    }
}

/// One request read off `stream`.
#[derive(Debug, PartialEq)]
struct Request {
    method: String,
    path: String,
    query: String,
    agent: String,
    body: Vec<u8>,
}

/// The request on `stream`: its line, the headers it needs, and a body of at
/// most [`MAX_BODY`]. `None` for anything malformed or too long.
fn read_request(stream: &mut impl Read) -> Option<Request> {
    let mut reader = BufReader::new(stream.take((MAX_HEAD + MAX_BODY) as u64));
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut words = line.split_whitespace();
    let method = words.next()?.to_string();
    let target = words.next()?;
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let (path, query) = (path.to_string(), query.to_string());
    let (mut length, mut agent, mut head) = (0usize, String::new(), line.len());
    loop {
        let mut h = String::new();
        let n = reader.read_line(&mut h).ok()?;
        head += n;
        if n == 0 || head > MAX_HEAD {
            return None;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        let (name, value) = h.split_once(':')?;
        match name.trim().to_ascii_lowercase().as_str() {
            "content-length" => length = value.trim().parse().ok()?,
            "user-agent" => agent = value.trim().to_string(),
            _ => {}
        }
    }
    if length > MAX_BODY {
        return None;
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).ok()?;
    Some(Request {
        method,
        path,
        query,
        agent,
        body,
    })
}

/// Answer one connection.
fn answer(mut stream: TcpStream, shared: &Mutex<Shared>, wake: &(dyn Fn() + Send + Sync)) {
    // An accepted socket may inherit the listener's non-blocking mode.
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(IO_WAIT));
    let _ = stream.set_write_timeout(Some(IO_WAIT));
    let Some(req) = read_request(&mut stream) else {
        let _ = reply(&mut stream, "400 Bad Request", "text/plain", b"");
        return;
    };
    let token = lock(shared).token.clone();
    let rest = req
        .path
        .strip_prefix('/')
        .and_then(|p| p.strip_prefix(token.as_str()));
    let _ = match (req.method.as_str(), rest) {
        ("GET", Some("")) => {
            let head = format!(
                "HTTP/1.1 301 Moved Permanently\r\nLocation: /{token}/\r\n\
                 Content-Length: 0\r\nConnection: close\r\n\r\n"
            );
            stream.write_all(head.as_bytes())
        }
        ("GET", Some("/")) => {
            let page = lock(shared).page.clone();
            reply(&mut stream, "200 OK", "text/html; charset=utf-8", &page)
        }
        ("GET", Some("/doc")) => {
            let doc = lock(shared).doc.clone();
            reply(&mut stream, "200 OK", "application/octet-stream", &doc)
        }
        ("GET", Some("/events")) => {
            let id = query_value(&req.query, "id").unwrap_or_default();
            if id.is_empty() || id.len() > 64 {
                reply(&mut stream, "400 Bad Request", "text/plain", b"")
            } else {
                listen(stream, shared, &id, &req.agent);
                wake();
                Ok(())
            }
        }
        ("POST", Some("/status")) => {
            if let Some((id, screen)) = status(&req.body) {
                let mut s = lock(shared);
                s.phone(&id, &req.agent).screen = screen;
                drop(s);
                wake();
            }
            reply(&mut stream, "204 No Content", "text/plain", b"")
        }
        _ => reply(&mut stream, "404 Not Found", "text/plain", b""),
    };
}

/// Open `id`'s stream of versions on `stream`, starting with the current one.
fn listen(mut stream: TcpStream, shared: &Mutex<Shared>, id: &str, agent: &str) {
    let mut s = lock(shared);
    let head = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
         Cache-Control: no-store\r\nConnection: keep-alive\r\n\r\ndata: {}\n\n",
        s.version
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    s.phone(id, agent).live = true;
    s.streams.push((id.to_string(), stream));
}

/// A whole response with a body, written a [`PIECE`] at a time.
fn reply(stream: &mut impl Write, status: &str, kind: &str, body: &[u8]) -> std::io::Result<()> {
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes())?;
    for piece in body.chunks(PIECE) {
        stream.write_all(piece)?;
    }
    stream.flush()
}

/// `name`'s value in a query string, undecoded beyond `%2D`-free ids.
fn query_value(query: &str, name: &str) -> Option<String> {
    query
        .split('&')
        .filter_map(|kv| kv.split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.to_string())
}

/// A status report's phone id and screen name.
fn status(body: &[u8]) -> Option<(String, String)> {
    let v: serde_json::Value = serde_json::from_slice(body).ok()?;
    let id = v.get("id")?.as_str()?.to_string();
    let screen = v.get("screen")?.as_str()?.chars().take(200).collect();
    (!id.is_empty() && id.len() <= 64).then_some((id, screen))
}

/// Whether `ip` may follow a share: a private, link-local or loopback address,
/// never a public one.
pub(crate) fn allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => v.is_private() || v.is_link_local() || v.is_loopback(),
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return allowed(IpAddr::V4(v4));
            }
            let first = v.segments()[0];
            v.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

/// This computer's address on its private network: the one its default route
/// leaves from, read without sending anything. `None` off a network or on a
/// public address only.
fn local_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("10.255.255.255:9").ok()?;
    let ip = socket.local_addr().ok()?.ip();
    match ip {
        IpAddr::V4(v) if v.is_private() => Some(ip),
        _ => None,
    }
}

/// A share's token: 128 random bits as 22 letters and digits, new each share,
/// so an old link or a photo of the code is dead.
fn token() -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    let mut n = (u128::from(crate::session::random_actor()) << 64)
        | u128::from(crate::session::random_actor());
    (0..22)
        .map(|_| {
            let c = ALPHABET[(n % 62) as usize] as char;
            n /= 62;
            c
        })
        .collect()
}

/// A browser named at its coarsest from its user agent: the device, then the
/// browser, *Android · Chrome*, *iPhone · Safari*.
pub(crate) fn browser_name(agent: &str) -> String {
    let device = if agent.contains("iPhone") {
        "iPhone"
    } else if agent.contains("iPad") {
        "iPad"
    } else if agent.contains("Android") {
        "Android"
    } else if agent.contains("Windows") {
        "Windows"
    } else if agent.contains("Mac OS X") || agent.contains("Macintosh") {
        "Mac"
    } else if agent.contains("Linux") {
        "Linux"
    } else {
        "Phone"
    };
    let browser = if agent.contains("Edg") {
        "Edge"
    } else if agent.contains("Firefox") || agent.contains("FxiOS") {
        "Firefox"
    } else if agent.contains("SamsungBrowser") {
        "Samsung Internet"
    } else if agent.contains("Chrome") || agent.contains("CriOS") {
        "Chrome"
    } else if agent.contains("Safari") {
        "Safari"
    } else {
        return device.to_string();
    };
    format!("{device} · {browser}")
}

// ── The player's share ──────────────────────────────────────────────────────

use crate::theme::{self, color};
use crate::ui;
use eframe::egui;
use mastik_core::{Document, NodeId};
use mastik_export::prototype::{Opening, Parts, Player};

/// How long the document must sit still after an edit before the phones are
/// sent it: a drag's commits land as one version, not dozens.
const SETTLE: f64 = 0.25;
/// How long a share waits for its first phone before the card's empty list
/// says where to look.
const UNREACHED: Duration = Duration::from_secs(20);
/// The most phones the card lists; the rest fold into a *+N* row.
const LISTED: usize = 4;
/// The QR code's modules and ground: dark on light whatever the theme, since a
/// camera reads the code, not a person; drawn as artwork, not chrome.
const QR_INK: egui::Color32 = egui::Color32::BLACK;
const QR_GROUND: egui::Color32 = egui::Color32::WHITE;
/// The light margin round the code, in modules, that the QR standard asks for.
const QR_QUIET: usize = 2;

/// *Play on a phone* as the player holds it: the share, or why there is
/// none, and what it last sent the phones.
pub(crate) struct Sharing {
    share: Result<Share, String>,
    /// The browser player's files, kept to write each new page.
    wasm: Vec<u8>,
    glue: String,
    opening: Opening,
    /// The fonts the page carries, as a count and their bytes, to tell an edit
    /// that needs the page loaded again from one the page can play.
    fonts: (usize, usize),
    version: u64,
    /// egui's clock when the document last changed without being sent.
    owed: Option<f64>,
    /// The link's QR code: its width in modules and each module, dark or not.
    qr: Option<(usize, Vec<bool>)>,
    /// The card is open in the player's side column.
    pub(crate) open: bool,
    /// When the share began serving, for the note a phone that never arrives
    /// gets.
    started: Option<Instant>,
}

/// What the sharing card asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CardAct {
    Retry,
    Stop,
    Copied,
}

/// One version of what the phones are sent.
struct Built {
    /// The fonts the page carries, as [`Sharing::fonts`] counts them.
    fonts: (usize, usize),
    page: Vec<u8>,
    doc: Vec<u8>,
}

/// The parts for `opening`: the page written for `version` and the document.
fn build(
    doc: &Document,
    wasm: &[u8],
    glue: &str,
    opening: Opening,
    version: u64,
) -> Result<Built, String> {
    let parts = Parts::mirror(doc, Player { wasm, glue }, opening)
        .map_err(|e| format!("Can't share this prototype: {e}"))?;
    let fonts = (
        parts.fonts.len(),
        parts.fonts.iter().map(|f| f.bytes.len()).sum(),
    );
    let frame = match opening {
        Opening::Frame(f) => Some(f),
        Opening::Flow(_) => None::<NodeId>,
    };
    let page = parts.mirror_html(frame, version).into_bytes();
    Ok(Built {
        fonts,
        page,
        doc: parts.document,
    })
}

impl Sharing {
    /// Start sharing `doc` from `opening`, repainting `ctx` as phones come and
    /// go. A share that cannot start is kept with its reason, for the card.
    pub(crate) fn start(ctx: &egui::Context, doc: &Document, opening: Opening) -> Sharing {
        let mut s = Sharing::stopped(opening);
        s.share = s.begin(ctx, doc);
        s
    }

    /// [`Sharing::start`] with the player's `files`, on `ip`.
    #[cfg(test)]
    pub(crate) fn start_with(
        ctx: &egui::Context,
        doc: &Document,
        opening: Opening,
        files: (Vec<u8>, String),
        ip: Option<IpAddr>,
    ) -> Sharing {
        let mut s = Sharing::stopped(opening);
        s.share = s.begin_with(ctx, doc, files, ip);
        s
    }

    /// A share not yet begun, its card open.
    fn stopped(opening: Opening) -> Sharing {
        Sharing {
            share: Err(String::new()),
            wasm: Vec::new(),
            glue: String::new(),
            opening,
            fonts: (0, 0),
            version: 1,
            owed: None,
            qr: None,
            open: true,
            started: None,
        }
    }

    /// The share, or why there is none.
    fn begin(&mut self, ctx: &egui::Context, doc: &Document) -> Result<Share, String> {
        let files = crate::player_build(doc).map_err(|_| {
            "The browser player isn't built, so there is nothing to serve".to_string()
        })?;
        self.begin_with(ctx, doc, files, local_ip())
    }

    /// [`Sharing::begin`] with the player's `files`, on `ip` (`None` off a
    /// network).
    fn begin_with(
        &mut self,
        ctx: &egui::Context,
        doc: &Document,
        (wasm, glue): (Vec<u8>, String),
        ip: Option<IpAddr>,
    ) -> Result<Share, String> {
        let Built {
            fonts,
            page,
            doc: bytes,
        } = build(doc, &wasm, &glue, self.opening, self.version)?;
        let wake = {
            let ctx = ctx.clone();
            move || ctx.request_repaint()
        };
        let started = match ip {
            Some(ip) => Share::start_on(ip, page, bytes, self.version, wake),
            None => Err(Refused::NoNetwork),
        };
        let share = started.map_err(|e| match e {
            Refused::NoNetwork => {
                "Not on a network. Connect this computer to Wi-Fi or a cable, and the code \
                 appears here"
                    .to_string()
            }
            Refused::Bind(why) => format!("Can't listen on this network: {why}"),
        })?;
        self.qr = qrcode::QrCode::new(share.url.as_bytes()).ok().map(|code| {
            let w = code.width();
            let dark = code
                .to_colors()
                .into_iter()
                .map(|c| c == qrcode::Color::Dark)
                .collect();
            (w, dark)
        });
        (self.wasm, self.glue, self.fonts) = (wasm, glue, fonts);
        self.started = Some(Instant::now());
        Ok(share)
    }

    /// Try again after a refusal.
    pub(crate) fn retry(&mut self, ctx: &egui::Context, doc: &Document) {
        self.share = self.begin(ctx, doc);
    }

    /// Whether the phones are being served.
    pub(crate) fn is_sharing(&self) -> bool {
        self.share.is_ok()
    }

    /// How many phones are listening now, for the bar's count.
    pub(crate) fn following(&self) -> usize {
        self.share
            .as_ref()
            .map_or(0, |s| s.phones().iter().filter(|p| p.live).count())
    }

    /// The document changed at `now` (egui's clock).
    pub(crate) fn changed(&mut self, now: f64) {
        self.owed = Some(now);
    }

    /// Send the phones `doc` once it has sat still for [`SETTLE`]: as a new
    /// version they play where they are, or as a new page when it needs fonts
    /// the page lacks. Asks for a repaint while one is owed.
    pub(crate) fn send(&mut self, ctx: &egui::Context, doc: &Document, now: f64) {
        let (Some(at), Ok(share)) = (self.owed, &self.share) else {
            return;
        };
        if now - at < SETTLE {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SETTLE));
            return;
        }
        self.owed = None;
        let version = self.version + 1;
        // An edit the trim refuses (the flow playing deleted) leaves the phones
        // where they are.
        let Ok(Built {
            fonts,
            page,
            doc: bytes,
        }) = build(doc, &self.wasm, &self.glue, self.opening, version)
        else {
            return;
        };
        self.version = version;
        if fonts == self.fonts {
            share.publish(bytes, version);
        } else {
            self.fonts = fonts;
            share.reload(page, bytes, version);
        }
    }

    /// The card in the player's side column, `ui`'s width: the code, the link,
    /// the phones following and *Stop sharing*; or why it cannot share. `doc` is
    /// the played document, whose screens the phones name.
    pub(crate) fn card(&self, ui: &mut egui::Ui, doc: &Document) -> Option<CardAct> {
        let mut act = None;
        let w = ui.available_width();
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.label(ui::eyebrow("Play on a phone"));
        let share = match &self.share {
            Ok(share) => share,
            Err(why) => {
                ui::empty_note(ui, why);
                let b = ui::label_button(
                    ui,
                    "Try again",
                    egui::vec2(w, ui::CONTROL_H),
                    ui::FieldButton::Off,
                );
                if b.clicked() {
                    act = Some(CardAct::Retry);
                }
                return act;
            }
        };
        if let Some((n, dark)) = &self.qr {
            let side = w.min(180.0);
            let (rect, _) = ui.allocate_exact_size(egui::vec2(w, side), egui::Sense::hover());
            let rect = egui::Rect::from_center_size(rect.center(), egui::vec2(side, side));
            let painter = ui.painter_at(rect);
            painter.rect_filled(rect, 4.0, QR_GROUND);
            let cell = side / (*n + 2 * QR_QUIET) as f32;
            for (i, on) in dark.iter().enumerate() {
                if *on {
                    let (x, y) = ((i % n + QR_QUIET) as f32, (i / n + QR_QUIET) as f32);
                    let at = rect.min + egui::vec2(x * cell, y * cell);
                    // A hair over, so neighbouring modules leave no seam.
                    painter.rect_filled(
                        egui::Rect::from_min_size(at, egui::vec2(cell + 0.5, cell + 0.5)),
                        0.0,
                        QR_INK,
                    );
                }
            }
        }
        let help = match cfg!(windows) {
            // The first share raises the system's firewall prompt; said before it
            // appears, and on every share, so the card keeps one size.
            true => {
                "Scan with the phone's camera. It must be on the same Wi-Fi as this computer. \
                 Windows may ask to let Mastik on private networks: allow it so the phone can \
                 reach this computer."
            }
            false => "Scan with the phone's camera. It must be on the same Wi-Fi as this computer.",
        };
        ui.add(
            egui::Label::new(
                egui::RichText::new(help)
                    .size(ui::SMALL_PT)
                    .color(theme::text::dim()),
            )
            .wrap(),
        );
        ui.horizontal(|ui| {
            let copy_w = 56.0;
            ui.add_sized(
                egui::vec2(w - copy_w - 6.0, ui::CONTROL_H),
                egui::Label::new(
                    egui::RichText::new(&share.url)
                        .size(ui::SMALL_PT)
                        .color(theme::text::muted()),
                )
                .truncate(),
            )
            .on_hover_text(&share.url);
            let b = ui::label_button(
                ui,
                "Copy",
                egui::vec2(copy_w, ui::CONTROL_H),
                ui::FieldButton::Off,
            );
            if b.clicked() {
                ui.ctx().copy_text(share.url.clone());
                act = Some(CardAct::Copied);
            }
        });
        ui.label(ui::eyebrow("Following"));
        let phones = share.phones();
        // The list keeps one height whatever it holds, so the card never jumps.
        let row_h = ui::MENU_ROW_H;
        let (list, _) =
            ui.allocate_exact_size(egui::vec2(w, row_h * LISTED as f32), egui::Sense::hover());
        let mut rows = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(list)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        rows.spacing_mut().item_spacing.y = 0.0;
        if phones.is_empty() {
            // A declined firewall prompt cannot be seen from here: a phone that
            // never arrives is the sign, so the note says where to look.
            let waited = self.started.is_some_and(|t| t.elapsed() >= UNREACHED);
            let note = match (cfg!(windows) && waited, waited) {
                (true, _) => {
                    "No phone yet. If the page won't open on the phone, allow Mastik on private \
                     networks in Windows Security, then try again"
                }
                (false, true) => {
                    "No phone yet. If the page won't open on the phone, check that both are on \
                     the same network"
                }
                _ => "No phone yet",
            };
            ui::empty_note(&mut rows, note);
            if waited {
                ui.ctx().request_repaint_after(Duration::from_secs(1));
            } else {
                ui.ctx().request_repaint_after(UNREACHED);
            }
        }
        let shown = if phones.len() > LISTED {
            LISTED - 1
        } else {
            LISTED
        };
        for p in phones.iter().take(shown) {
            phone_row(&mut rows, w, row_h, p, doc);
        }
        if phones.len() > LISTED {
            ui::more_row(&mut rows, row_h, &format!("+{}", phones.len() - shown));
        }
        let b = ui::label_button(
            ui,
            "Stop sharing",
            egui::vec2(w, ui::CONTROL_H),
            ui::FieldButton::Danger,
        );
        if b.clicked() {
            act = Some(CardAct::Stop);
        }
        act
    }
}

/// A phone's reported frame name as drawn. The page reports the name alone, so
/// private mode finds the screen of that name in the played `doc` and draws
/// its masked name, or *Frame* when none has it.
fn screen_shown<'a>(doc: &'a Document, screen: &'a str) -> std::borrow::Cow<'a, str> {
    if !crate::private::on() {
        return std::borrow::Cow::Borrowed(screen);
    }
    doc.page_ids()
        .filter_map(|p| doc.get(p))
        .flat_map(|p| p.children().iter().copied())
        .find(|&id| doc.get(id).is_some_and(|n| n.name() == screen))
        .map_or(std::borrow::Cow::Borrowed("Frame"), |id| {
            crate::private::layer(doc, id)
        })
}

/// One phone in the card's list: a dot (lit while it listens, grey while it
/// sleeps), its browser and the frame it shows.
fn phone_row(ui: &mut egui::Ui, w: f32, h: f32, p: &Phone, doc: &Document) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let painter = ui.painter_at(rect);
    let dot = match p.live {
        true => color::saved(),
        false => theme::text::faint(),
    };
    painter.circle_filled(egui::pos2(rect.left() + 4.0, rect.center().y), 3.0, dot);
    let name = painter.layout_no_wrap(
        p.name.clone(),
        egui::FontId::proportional(ui::SMALL_PT),
        theme::text::strong(),
    );
    let name_w = name.size().x;
    painter.galley(
        egui::pos2(rect.left() + 14.0, rect.center().y - name.size().y / 2.0),
        name,
        theme::text::strong(),
    );
    let room = rect.right() - (rect.left() + 14.0 + name_w + 8.0);
    if room > 16.0 && !p.screen.is_empty() {
        let mut job = egui::text::LayoutJob::simple_singleline(
            screen_shown(doc, &p.screen).into_owned(),
            egui::FontId::proportional(ui::SMALL_PT),
            theme::text::dim(),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let screen = painter.layout_job(job);
        painter.galley(
            egui::pos2(
                rect.right() - screen.size().x,
                rect.center().y - screen.size().y / 2.0,
            ),
            screen,
            theme::text::dim(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    /// One request to `share` on loopback, answered whole.
    fn get(share: &Share, request: &str) -> String {
        let addr = share.url.trim_start_matches("http://");
        let host = addr.split('/').next().expect("host");
        let mut s = TcpStream::connect(host).expect("connect");
        s.write_all(request.as_bytes()).expect("send");
        let mut out = String::new();
        let _ = s.read_to_string(&mut out);
        out
    }

    fn share() -> Share {
        Share::start_on(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            b"<html>page</html>".to_vec(),
            b"DOC1".to_vec(),
            1,
            || {},
        )
        .expect("a share on loopback")
    }

    fn token_of(share: &Share) -> String {
        share.url.rsplit('/').nth(1).expect("token").to_string()
    }

    /// The page and the document are served under the token alone; any other
    /// path, or another token, is not found.
    ///
    /// flip: `answer` not checking the token → fails at "another token".
    #[test]
    fn the_page_and_document_are_served_under_the_token() {
        let share = share();
        let t = token_of(&share);
        let page = get(&share, &format!("GET /{t}/ HTTP/1.1\r\n\r\n"));
        assert!(page.starts_with("HTTP/1.1 200"), "{page}");
        assert!(page.ends_with("<html>page</html>"), "the page");
        let doc = get(&share, &format!("GET /{t}/doc?v=1 HTTP/1.1\r\n\r\n"));
        assert!(doc.ends_with("DOC1"), "the document");
        let moved = get(&share, &format!("GET /{t} HTTP/1.1\r\n\r\n"));
        assert!(moved.contains(&format!("Location: /{t}/")), "{moved}");
        let other = get(&share, "GET /AAAAAAAAAAAAAAAAAAAAAA/ HTTP/1.1\r\n\r\n");
        assert!(other.starts_with("HTTP/1.1 404"), "another token: {other}");
    }

    /// A phone that opens the stream hears the current version, then each one
    /// published; its status names its screen; when the share stops it hears
    /// `stop`.
    ///
    /// flip: `Share::publish` not sending to the streams → fails at "a line",
    /// the read timing out where the new version should be.
    #[test]
    fn a_phone_hears_each_version_and_the_stop() {
        let share = share();
        let t = token_of(&share);
        let host = share
            .url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let mut events = TcpStream::connect(&host).expect("connect");
        events
            .write_all(
                format!(
                    "GET /{t}/events?id=p1 HTTP/1.1\r\nUser-Agent: Mozilla/5.0 (Linux; \
                     Android 14) Chrome/130.0 Mobile Safari/537.36\r\n\r\n"
                )
                .as_bytes(),
            )
            .unwrap();
        events
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(events);
        let mut next_event = || loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("a line");
            if let Some(d) = line.trim_end().strip_prefix("data: ") {
                return d.to_string();
            }
        };
        assert_eq!(next_event(), "1", "the current version first");
        let body = r#"{"id":"p1","screen":"Cart"}"#;
        let ok = get(
            &share,
            &format!(
                "POST /{t}/status HTTP/1.1\r\nContent-Length: {}\r\n\r\n{body}",
                body.len()
            ),
        );
        assert!(ok.starts_with("HTTP/1.1 204"), "{ok}");
        let phones = share.phones();
        assert_eq!(phones.len(), 1);
        assert_eq!(phones[0].name, "Android · Chrome");
        assert_eq!(phones[0].screen, "Cart");
        assert!(phones[0].live, "listening");
        share.publish(b"DOC2".to_vec(), 2);
        assert_eq!(next_event(), "2", "hears the new version");
        drop(share);
        assert_eq!(next_event(), "stop", "hears the stop");
    }

    /// Only private, link-local and loopback addresses may follow.
    #[test]
    fn only_the_private_network_is_allowed() {
        for ip in [
            "192.168.1.24",
            "10.0.0.5",
            "172.16.3.1",
            "127.0.0.1",
            "169.254.1.1",
            "fd00::1",
            "fe80::1",
            "::1",
            "::ffff:192.168.0.2",
        ] {
            assert!(allowed(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "172.32.0.1", "2001:db8::1", "::ffff:8.8.8.8"] {
            assert!(!allowed(ip.parse().unwrap()), "{ip}");
        }
    }

    /// A browser is named by its device and browser, Chrome on iOS as Chrome
    /// and Edge before the Chrome its agent also names.
    #[test]
    fn a_browser_is_named_at_its_coarsest() {
        let iphone = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) AppleWebKit/605.1.15 \
                      (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1";
        assert_eq!(browser_name(iphone), "iPhone · Safari");
        let crios = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X) CriOS/130.0 Mobile Safari/604.1";
        assert_eq!(browser_name(crios), "iPhone · Chrome");
        let edge = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) Chrome/130.0 Safari/537.36 Edg/130.0";
        assert_eq!(browser_name(edge), "Windows · Edge");
    }

    /// An edit reaches a following phone as a new version once the document
    /// has sat still, and the document it fetches carries the edit.
    ///
    /// flip: `Sharing::send` not waiting for the document to settle → fails at
    /// "nothing before it settles".
    #[test]
    fn an_edit_reaches_the_phone_once_it_settles() {
        use mastik_core::kurbo::{Affine, RoundedRectRadii, Size};
        use mastik_core::{IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0x91);
        let page = ids.mint();
        let mut doc = Document::new(page);
        let frame = ids.mint();
        let make = |id, name: &str| Operation::CreateNode {
            id,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 80.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some(name.into()),
        };
        doc.apply(&Transaction(vec![make(frame, "Home")]))
            .expect("a frame");
        let ctx = egui::Context::default();
        let opening = Opening::Frame(frame);
        let mut s = Sharing::start_with(
            &ctx,
            &doc,
            opening,
            (b"\0asm".to_vec(), "let wasm_bindgen;".into()),
            Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        );
        let share = s.share.as_ref().expect("sharing on loopback");
        let host = share
            .url
            .trim_start_matches("http://")
            .split('/')
            .next()
            .unwrap()
            .to_string();
        let t = share.url.rsplit('/').nth(1).unwrap().to_string();
        let mut events = TcpStream::connect(&host).expect("connect");
        events
            .write_all(format!("GET /{t}/events?id=p HTTP/1.1\r\n\r\n").as_bytes())
            .unwrap();
        events
            .set_read_timeout(Some(Duration::from_millis(800)))
            .unwrap();
        let mut reader = BufReader::new(events);
        let mut next_event = || loop {
            let mut line = String::new();
            reader.read_line(&mut line).ok()?;
            if let Some(d) = line.trim_end().strip_prefix("data: ") {
                return Some(d.to_string());
            }
        };
        assert_eq!(next_event().as_deref(), Some("1"), "fixture: version 1");
        let other = ids.mint();
        doc.apply(&Transaction(vec![make(other, "Added")]))
            .expect("an edit");
        s.changed(10.0);
        s.send(&ctx, &doc, 10.1);
        assert_eq!(next_event(), None, "nothing before it settles");
        s.send(&ctx, &doc, 10.0 + SETTLE);
        assert_eq!(next_event().as_deref(), Some("2"), "the new version");
        let mut req = TcpStream::connect(&host).unwrap();
        req.write_all(format!("GET /{t}/doc?v=2 HTTP/1.1\r\n\r\n").as_bytes())
            .unwrap();
        let mut out = Vec::new();
        let _ = req.read_to_end(&mut out);
        let at = out
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .expect("a head")
            + 4;
        let sent = mastik_core::io::load(&out[at..]).expect("the document loads");
        assert!(sent.get(other).is_some(), "carries the edit");
    }

    /// A request longer than the limit, or with a body over it, is refused.
    #[test]
    fn an_oversized_request_is_refused() {
        let long = format!("GET / HTTP/1.1\r\nX: {}\r\n\r\n", "a".repeat(MAX_HEAD));
        assert!(read_request(&mut long.as_bytes()).is_none(), "head");
        let body = format!(
            "POST / HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_BODY + 1
        );
        assert!(read_request(&mut body.as_bytes()).is_none(), "body");
        let ok = "GET /t/events?id=a HTTP/1.1\r\nUser-Agent: X\r\n\r\n";
        let req = read_request(&mut ok.as_bytes()).expect("read");
        assert_eq!(
            (req.path.as_str(), req.query.as_str(), req.agent.as_str()),
            ("/t/events", "id=a", "X")
        );
    }

    /// A reply hands the socket its body a `PIECE` at a time, whole and in
    /// order, so the write timeout bounds each piece rather than the body. Not a
    /// loopback test: Windows loopback takes a 16 MB send at once, so a slow
    /// reader there never trips the timeout with or without the pieces.
    /// flip: `reply` writing the body in one `write_all` → fails at "a piece".
    #[test]
    fn a_reply_is_written_a_piece_at_a_time() {
        /// Every byte written, and the longest single write.
        #[derive(Default)]
        struct Socket {
            got: Vec<u8>,
            longest: usize,
        }
        impl Write for Socket {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.longest = self.longest.max(buf.len());
                self.got.extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let body: Vec<u8> = (0..PIECE * 5 + 17).map(|i| (i % 251) as u8).collect();
        let mut socket = Socket::default();
        reply(&mut socket, "200 OK", "application/octet-stream", &body).expect("written");
        assert!(socket.longest <= PIECE, "a piece: {}", socket.longest);
        assert!(socket.got.ends_with(&body), "every byte, in order");
    }

    /// In private mode a phone's row draws the screen it reports masked, as the
    /// player's own card does.
    /// flip: `screen_shown` passing the reported name through → fails at "masked".
    #[test]
    fn a_phone_row_masks_its_screen_in_private_mode() {
        use mastik_core::kurbo::Size;
        use mastik_core::{IdSource, NodeKind, Operation, Transaction};
        let mut ids = IdSource::new(0x9A0);
        let root = ids.mint();
        let frame = ids.mint();
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 800.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Zqx pricing".into()),
        }]))
        .expect("a screen");
        let phone = Phone {
            id: "p1".into(),
            name: "Android · Chrome".into(),
            screen: "Zqx pricing".into(),
            live: true,
            seen: Instant::now(),
        };
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let drawn = || {
            let out = ctx.run_ui(Default::default(), |ui| {
                phone_row(ui, 320.0, ui::MENU_ROW_H, &phone, &doc);
            });
            out.shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_string()),
                    _ => None,
                })
                .collect::<Vec<_>>()
        };
        assert!(
            drawn().iter().any(|t| t == "Zqx pricing"),
            "control: the name"
        );
        crate::private::set(true);
        let masked = drawn();
        crate::private::set(false);
        assert!(
            !masked.iter().any(|t| t.contains("Zqx"))
                && masked.iter().any(|t| t.starts_with("Frame")),
            "masked: {masked:?}"
        );
    }
}
