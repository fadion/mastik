//! A host's MCP endpoint (§8.1): a local socket admitting only this user, one
//! thread per connection, and a discovery file the proxy finds it by.

use mastik_mcp::queue::{Conn, Forward, Lists, Request};
use mastik_mcp::wire;
use serde::{Deserialize, Serialize};
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::Sender;

/// Which kind of process hosts an endpoint; the proxy prefers the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Host {
    Window,
    Serve,
}

/// A discovery file's content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Found {
    pub pid: u32,
    pub host: Host,
    /// The socket's path, or the pipe's name.
    pub endpoint: PathBuf,
}

/// The folder holding discovery files and, off Windows, the sockets: beside the
/// window's lock.
pub(crate) fn run_dir() -> Option<PathBuf> {
    Some(crate::one_window::window_dir()?.join("mcp"))
}

/// The tool definitions a host lists: the core set, or with `all` every tool of
/// every phase. An unlisted tool still answers, through `call` or by name.
pub(crate) fn tools(all: bool) -> Vec<serde_json::Value> {
    use mastik_mcp::tools::{CORE, PHASE_1, PHASE_2, PHASE_3, PHASE_4};
    let names: Vec<&str> = match all {
        true => PHASE_1
            .iter()
            .chain(&PHASE_2)
            .chain(&PHASE_3)
            .chain(&PHASE_4)
            .copied()
            .collect(),
        false => CORE.to_vec(),
    };
    names
        .into_iter()
        .filter_map(mastik_mcp::schema::definition)
        .collect()
}

/// [`tools`] built once for every connection: the lists never change while
/// the process runs.
static LISTS: std::sync::LazyLock<[std::sync::Arc<Vec<serde_json::Value>>; 2]> =
    std::sync::LazyLock::new(|| [tools(false).into(), tools(true).into()]);

/// A listening endpoint; dropping it removes the discovery file and, off
/// Windows, the socket.
pub(crate) struct Endpoint {
    file: PathBuf,
    socket: Option<PathBuf>,
}

impl Drop for Endpoint {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.file);
        if let Some(socket) = &self.socket {
            let _ = std::fs::remove_file(socket);
        }
    }
}

/// Listen for this process as `host`, sending every call to `to`; `lists`, when
/// given, answers a granted peer's `tools/list`.
pub(crate) fn listen(
    host: Host,
    to: Sender<Request>,
    lists: Option<Lists>,
) -> std::io::Result<Endpoint> {
    let dir = run_dir().ok_or_else(|| std::io::Error::other("no folder for the endpoint"))?;
    make_private_dir(&dir)?;
    let pid = std::process::id();
    let endpoint = address(&dir, pid);
    serve_at(&endpoint, Accepted { to, lists })?;
    let file = dir.join(format!("{pid}.json"));
    let socket = cfg!(not(windows)).then(|| endpoint.clone());
    let found = Found {
        pid,
        host,
        endpoint,
    };
    let bytes = serde_json::to_vec(&found).map_err(std::io::Error::other)?;
    crate::atomic::write(&file, &bytes)?;
    Ok(Endpoint { file, socket })
}

/// Every discovery file in the run folder whose endpoint answers, the window's
/// first. A file whose process is gone is removed.
pub(crate) fn hosts() -> Vec<Found> {
    let Some(dir) = run_dir() else {
        return Vec::new();
    };
    hosts_in(
        &dir,
        |endpoint| connect(endpoint).is_ok(),
        mastik_ai::process_running,
    )
}

/// [`hosts`] over the discovery files in `dir`, with `answers` saying whether
/// an endpoint takes a connection and `running` whether a process lives.
fn hosts_in(
    dir: &Path,
    answers: impl Fn(&Path) -> bool,
    running: impl Fn(u32) -> bool,
) -> Vec<Found> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(Found, PathBuf)> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .filter_map(|p| {
            let f: Found = serde_json::from_slice(&std::fs::read(&p).ok()?).ok()?;
            Some((f, p))
        })
        .collect();
    // A failed connect is not a dead host: a busy pipe or a window at another
    // privilege refuses too. Only a file whose process is gone is removed, since
    // a window writes its file once and would stay unseen until it restarts.
    found.retain(|(f, file)| {
        if answers(&f.endpoint) {
            return true;
        }
        if !running(f.pid) {
            let _ = std::fs::remove_file(file);
        }
        false
    });
    found.sort_by_key(|(f, _)| (f.host != Host::Window, f.pid));
    found.into_iter().map(|(f, _)| f).collect()
}

/// A connection to an endpoint, as a reader and a writer.
pub(crate) type Stream = (Box<dyn Read + Send>, Box<dyn Write + Send>);

static NEXT: AtomicU64 = AtomicU64::new(1);

/// What every accepted connection is handed: where its calls go and the host's
/// own lists.
#[derive(Clone)]
struct Accepted {
    to: Sender<Request>,
    lists: Option<Lists>,
}

/// The most connections answered at once, each on a thread of its own: past
/// it a new one is closed unread, so a client looping on connects cannot take
/// the window's threads.
const MAX_CONNECTIONS: usize = 64;

/// The connections being answered now.
static OPEN: AtomicUsize = AtomicUsize::new(0);

/// Answer one accepted connection from process `pid` on its own thread.
fn spawn_connection(
    read: Box<dyn Read + Send>,
    write: Box<dyn Write + Send>,
    pid: Option<u32>,
    with: Accepted,
) {
    if OPEN.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
        OPEN.fetch_sub(1, Ordering::AcqRel);
        log::warn!(target: "mastik", "mcp: {MAX_CONNECTIONS} connections open; one refused");
        return;
    }
    struct Closed;
    impl Drop for Closed {
        fn drop(&mut self) {
            OPEN.fetch_sub(1, Ordering::AcqRel);
        }
    }
    let conn: Conn = NEXT.fetch_add(1, Ordering::Relaxed);
    let spawned = std::thread::Builder::new()
        .name(format!("mcp-{conn}"))
        .spawn(move || {
            let _closed = Closed;
            let mut handler = Forward {
                conn,
                to: with.to,
                pid,
                tools: LISTS[0].clone(),
                all_tools: LISTS[1].clone(),
                lists: with.lists,
                wait: mastik_mcp::queue::TAKE_WAIT,
            };
            let _ = wire::serve(BufReader::new(read), write, &mut handler);
        });
    // A thread that never started never drops its `Closed`.
    if spawned.is_err() {
        OPEN.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(windows)]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)
}

#[cfg(not(windows))]
fn make_private_dir(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

/// The pipe's name: per user (the folder holds the profile) and per process.
#[cfg(windows)]
fn address(dir: &Path, pid: u32) -> PathBuf {
    let user = crate::one_window::fnv1a(dir.as_os_str().as_encoded_bytes());
    PathBuf::from(format!(r"\\.\pipe\Mastik-mcp-{user:016x}-{pid}"))
}

#[cfg(not(windows))]
fn address(dir: &Path, pid: u32) -> PathBuf {
    dir.join(format!("{pid}.sock"))
}

/// Accept connections at `addr` on a thread of its own. The pipe rejects remote
/// clients, its default DACL lets only this user write, and a client that runs
/// as anyone else is dropped.
#[cfg(windows)]
fn serve_at(addr: &Path, with: Accepted) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt as _;
    let name: Vec<u16> = addr.as_os_str().encode_wide().chain([0]).collect();
    let first = crate::one_window::pipe_instance(&name, true)?;
    std::thread::Builder::new()
        .name("mcp-listen".into())
        .spawn(move || {
            let mut next = Some(first);
            while let Some(pipe) = next.take() {
                if let Ok(pipe) = crate::one_window::connected(pipe)
                    && let Some(pid) = this_users_client(&pipe)
                    && let Ok(read) = pipe.try_clone()
                {
                    spawn_connection(Box::new(read), Box::new(pipe), Some(pid), with.clone());
                }
                match crate::one_window::pipe_instance(&name, false) {
                    Ok(pipe) => next = Some(pipe),
                    Err(e) => log::warn!(target: "mastik", "mcp: pipe stopped: {e}"),
                }
            }
        })?;
    Ok(())
}

/// The connected client's process when it runs as this user, else `None`.
#[cfg(windows)]
fn this_users_client(pipe: &std::fs::File) -> Option<u32> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::System::Pipes::GetNamedPipeClientProcessId;
    let mut pid = 0u32;
    // SAFETY: `pipe` is a connected pipe handle for the whole call, `pid` writable.
    let ok = unsafe { GetNamedPipeClientProcessId(pipe.as_raw_handle() as _, &mut pid) } != 0;
    (ok && crate::one_window::same_user(pid)).then_some(pid)
}

#[cfg(not(windows))]
fn serve_at(addr: &Path, with: Accepted) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    let _ = std::fs::remove_file(addr);
    let listener = std::os::unix::net::UnixListener::bind(addr)?;
    std::fs::set_permissions(addr, std::fs::Permissions::from_mode(0o600))?;
    std::thread::Builder::new()
        .name("mcp-listen".into())
        .spawn(move || {
            for stream in listener.incoming() {
                // An accept that keeps failing (out of descriptors) would
                // otherwise spin this thread.
                let stream = match stream {
                    Ok(s) => s,
                    Err(e) => {
                        log::warn!(target: "mastik", "mcp: accept failed: {e}");
                        std::thread::sleep(std::time::Duration::from_millis(100));
                        continue;
                    }
                };
                if !peer_is_this_user(&stream) {
                    continue;
                }
                let pid = peer_pid(&stream);
                if let Ok(read) = stream.try_clone() {
                    spawn_connection(Box::new(read), Box::new(stream), pid, with.clone());
                }
            }
        })?;
    Ok(())
}

/// The connected peer's process, from `SO_PEERCRED`.
#[cfg(target_os = "linux")]
fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are writable and sized for `SO_PEERCRED`.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    } == 0;
    (ok && cred.pid > 0).then_some(cred.pid as u32)
}

/// The connected peer's process, from `LOCAL_PEERPID`. The two values are
/// macOS's `<sys/un.h>`, written here so no `libc` version decides the build.
#[cfg(target_os = "macos")]
fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<u32> {
    use std::os::fd::AsRawFd as _;
    const SOL_LOCAL: libc::c_int = 0;
    const LOCAL_PEERPID: libc::c_int = 0x002;
    let mut pid: libc::pid_t = 0;
    let mut len = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `len` are writable and sized for `LOCAL_PEERPID`.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            SOL_LOCAL,
            LOCAL_PEERPID,
            (&raw mut pid).cast(),
            &mut len,
        )
    } == 0;
    (ok && pid > 0).then_some(pid as u32)
}

/// No peer process on the other Unixes: only a key grants a level there.
#[cfg(all(unix, not(any(target_os = "linux", target_os = "macos"))))]
fn peer_pid(_: &std::os::unix::net::UnixStream) -> Option<u32> {
    None
}

#[cfg(target_os = "linux")]
fn peer_is_this_user(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    let mut cred = libc::ucred {
        pid: 0,
        uid: u32::MAX,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // SAFETY: `cred` and `len` are writable and sized for `SO_PEERCRED`.
    let ok = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&raw mut cred).cast(),
            &mut len,
        )
    } == 0;
    // SAFETY: takes no arguments.
    ok && cred.uid == unsafe { libc::getuid() }
}

#[cfg(all(unix, not(target_os = "linux")))]
fn peer_is_this_user(stream: &std::os::unix::net::UnixStream) -> bool {
    use std::os::fd::AsRawFd as _;
    let (mut uid, mut gid) = (u32::MAX, 0);
    // SAFETY: `uid` and `gid` are writable.
    let ok = unsafe { libc::getpeereid(stream.as_raw_fd(), &mut uid, &mut gid) } == 0;
    // SAFETY: takes no arguments.
    ok && uid == unsafe { libc::getuid() }
}

/// Connect to the endpoint at `addr`. Every pipe instance is busy for a moment
/// after a client takes one, until the host makes the next, so that is retried.
/// A pipe served by another user's process is refused: a dead host's name is
/// predictable, and whoever took it would read the agent's calls.
#[cfg(windows)]
pub(crate) fn connect(addr: &Path) -> std::io::Result<Stream> {
    use std::os::windows::io::AsRawHandle as _;
    use windows_sys::Win32::Foundation::ERROR_PIPE_BUSY;
    use windows_sys::Win32::System::Pipes::GetNamedPipeServerProcessId;
    let started = std::time::Instant::now();
    let pipe = loop {
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(addr)
        {
            Err(e)
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY as i32)
                    && started.elapsed() < std::time::Duration::from_secs(2) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => break other?,
        }
    };
    let mut pid = 0u32;
    // SAFETY: `pipe` is an open pipe handle for the whole call, `pid` writable.
    if unsafe { GetNamedPipeServerProcessId(pipe.as_raw_handle() as _, &mut pid) } == 0
        || !crate::one_window::same_user(pid)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "the pipe is not served by this user's Mastik",
        ));
    }
    let read = pipe.try_clone()?;
    Ok((Box::new(read), Box::new(pipe)))
}

#[cfg(not(windows))]
pub(crate) fn connect(addr: &Path) -> std::io::Result<Stream> {
    let stream = std::os::unix::net::UnixStream::connect(addr)?;
    let read = stream.try_clone()?;
    Ok((Box::new(read), Box::new(stream)))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The sweep lists the files whose endpoint answers, the window's first,
    /// keeps a refusing file whose process lives, and removes only a file whose
    /// process is gone.
    ///
    /// flip: `hosts_in` removing a file on a failed connect alone → fails at
    /// "stays on disk".
    #[test]
    fn the_sweep_removes_only_a_dead_hosts_file() {
        let dir = std::env::temp_dir().join(format!("mastik-endpoint-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, pid: u32, host: Host| {
            let found = Found {
                pid,
                host,
                endpoint: PathBuf::from(name),
            };
            let file = dir.join(format!("{name}.json"));
            std::fs::write(&file, serde_json::to_vec(&found).unwrap()).unwrap();
            file
        };
        let serve = write("serve", 1, Host::Serve);
        let window = write("window", 2, Host::Window);
        let busy = write("busy", 3, Host::Window);
        let dead = write("dead", 4, Host::Serve);
        let listed = hosts_in(
            &dir,
            |endpoint| endpoint == Path::new("serve") || endpoint == Path::new("window"),
            |pid| pid != 4,
        );
        let pids: Vec<u32> = listed.iter().map(|f| f.pid).collect();
        assert_eq!(pids, [2, 1], "the answering files, the window's first");
        assert!(busy.exists(), "a refusing live host's file stays on disk");
        assert!(!dead.exists(), "a dead host's file is removed");
        assert!(serve.exists() && window.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
