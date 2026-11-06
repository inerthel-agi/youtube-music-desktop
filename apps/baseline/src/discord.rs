//! Discord Rich Presence over Discord's local named pipes.
//!
//! - Every running Discord client gets the activity: Stable, PTB and Canary each
//!   open their own pipe (`discord-ipc-0..9`), so the worker keeps one connection
//!   per pipe instead of stopping at the first one.
//! - Switching account or restarting Discord closes the pipe, and Discord also
//!   drops connections by itself now and then (which clears the activity). While
//!   a track is shown, the worker checks its connections every `HEALTH` and
//!   reconnects, then sends the current activity again. A send that fails
//!   reconnects and resends at once. With nothing to show it just blocks on its
//!   channel (no timer at all).
//! - A pipe name proves nothing: another Windows account can create
//!   `discord-ipc-N` while Discord is not running. The activity is sent only to a
//!   pipe server running as this same user that answers the handshake with READY.

use std::{
    fs::{File, OpenOptions},
    io::{self, Read, Write},
    os::windows::io::AsRawHandle,
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread,
    time::{Duration, Instant},
};

use windows::Win32::{
    Foundation::{CloseHandle, HANDLE},
    Security::{EqualSid, GetTokenInformation, PSID, TOKEN_QUERY, TOKEN_USER, TokenUser},
    System::{
        Pipes::{GetNamedPipeServerProcessId, PeekNamedPipe},
        Threading::{GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION},
    },
};
use ytm_core::discord::{
    Activity, CLIENT_ID, OP_FRAME, OP_HANDSHAKE, decode_header, encode, handshake, set_activity,
};

/// The playing track; `None` clears the activity.
pub type Track = Option<Activity>;

enum Msg {
    Update(Track),
    Stop,
}

pub struct Presence {
    tx: Sender<Msg>,
}

impl Presence {
    pub fn start() -> Self {
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new()
            .name("discord-rpc".into())
            .spawn(move || run(rx));
        if let Err(e) = spawned {
            log::error!("cannot start Discord worker: {e}");
        }
        Self { tx }
    }

    pub fn update(&self, track: Track) {
        let _ = self.tx.send(Msg::Update(track));
    }
}

impl Drop for Presence {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
    }
}

/// A track change arrives as a burst (title first, then duration, play, ...):
/// wait until it settles and send only the latest state.
const SETTLE: Duration = Duration::from_millis(1200);
/// Discord accepts about 5 activity updates per 20 s and silently drops the
/// rest, which left stale or incomplete timings on screen.
const MIN_GAP: Duration = Duration::from_secs(4);
/// Connection check / reconnect interval while a track is shown: the longest
/// time the activity stays missing after Discord drops the pipe. One
/// `PeekNamedPipe` per client, plus failed opens when Discord is not running.
const HEALTH: Duration = Duration::from_secs(5);
const OP_CLOSE: u32 = 2;
const OP_PING: u32 = 3;
const OP_PONG: u32 = 4;

enum Event {
    Msg(Msg),
    Tick,
}

/// Waits for the next message (or `idle` elapses: `Tick`), then keeps replacing
/// it with newer ones until the channel stays quiet for `SETTLE` and `MIN_GAP`
/// has passed since the last send.
fn next_settled(
    rx: &mpsc::Receiver<Msg>,
    last_sent: Option<Instant>,
    idle: Option<Duration>,
) -> Event {
    let first = match idle {
        Some(t) => rx.recv_timeout(t),
        None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
    };
    let mut msg = match first {
        Ok(m) => m,
        Err(RecvTimeoutError::Timeout) => return Event::Tick,
        Err(RecvTimeoutError::Disconnected) => return Event::Msg(Msg::Stop),
    };
    loop {
        if matches!(msg, Msg::Stop) {
            return Event::Msg(msg);
        }
        let gap = last_sent.map_or(Duration::ZERO, |t| MIN_GAP.saturating_sub(t.elapsed()));
        match rx.recv_timeout(SETTLE.max(gap)) {
            Ok(newer) => msg = newer,
            Err(RecvTimeoutError::Timeout) => return Event::Msg(msg),
            Err(RecvTimeoutError::Disconnected) => return Event::Msg(Msg::Stop),
        }
    }
}

struct Conn {
    index: u8,
    pipe: File,
    /// What this client currently shows; `None` = nothing sent yet.
    shown: Option<Track>,
}

fn run(rx: mpsc::Receiver<Msg>) {
    let pid = std::process::id();
    let mut conns: Vec<Conn> = Vec::new();
    let mut current: Track = None;
    let mut last_sent: Option<Instant> = None;
    let mut nonce = 0u64;
    loop {
        match next_settled(&rx, last_sent, current.is_some().then_some(HEALTH)) {
            Event::Msg(Msg::Stop) => {
                for c in &mut conns {
                    nonce += 1;
                    let _ = send(&mut c.pipe, &set_activity(pid, nonce, None));
                }
                return;
            }
            Event::Msg(Msg::Update(t)) => current = t,
            Event::Tick => {}
        }
        // Drop clients that went away (account switch, Discord restart, quit).
        conns.retain(|c| {
            let ok = alive(&c.pipe);
            if !ok {
                log::info!("Discord pipe {} closed", c.index);
            }
            ok
        });
        let mut sent = 0;
        // Second pass only after a failed send: reconnect and resend now rather
        // than at the next HEALTH tick.
        for _ in 0..2 {
            if current.is_some() {
                connect_missing(&mut conns);
            }
            let mut failed = false;
            conns.retain_mut(|c| {
                if c.shown.as_ref() == Some(&current) {
                    return true;
                }
                nonce += 1;
                match send(&mut c.pipe, &set_activity(pid, nonce, current.as_ref())) {
                    Ok(()) => {
                        c.shown = Some(current.clone());
                        sent += 1;
                        true
                    }
                    Err(e) => {
                        log::warn!("Discord pipe {}: update failed: {e}", c.index);
                        failed = true;
                        false
                    }
                }
            });
            if !failed {
                break;
            }
        }
        if sent > 0 {
            last_sent = Some(Instant::now());
            log::info!(
                "Discord activity {} ({sent} client(s))",
                if current.is_some() { "sent" } else { "cleared" }
            );
        }
    }
}

/// Opens every Discord pipe not connected yet (one per running client).
fn connect_missing(conns: &mut Vec<Conn>) {
    for index in 0..10u8 {
        if conns.iter().any(|c| c.index == index) {
            continue;
        }
        match connect(index) {
            Ok(pipe) => {
                log::info!("connected to Discord (pipe {index})");
                conns.push(Conn {
                    index,
                    pipe,
                    shown: None,
                });
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => log::debug!("Discord pipe {index}: {e}"),
        }
    }
}

fn connect(index: u8) -> io::Result<File> {
    let mut pipe = OpenOptions::new()
        .read(true)
        .write(true)
        .open(format!(r"\\.\pipe\discord-ipc-{index}"))?;
    if !server_is_same_user(&pipe) {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "pipe server runs as another Windows user",
        ));
    }
    pipe.write_all(&encode(OP_HANDSHAKE, &handshake(CLIENT_ID)))?;
    let (op, body) = read_frame(&mut pipe)?; // READY, or CLOSE with a reason
    if op == OP_CLOSE {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionRefused,
            String::from_utf8_lossy(&body).into_owned(),
        ));
    }
    let ready = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .is_some_and(|v| v.get("evt").and_then(|e| e.as_str()) == Some("READY"));
    if !ready {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "no READY from the pipe server"));
    }
    Ok(pipe)
}

/// The server end of `pipe` runs under the same Windows account as this app.
/// A process of another account cannot be opened or has a different user SID.
fn server_is_same_user(pipe: &File) -> bool {
    let mut pid = 0u32;
    // SAFETY: valid pipe handle owned by `pipe`; `pid` is a plain out value.
    if unsafe { GetNamedPipeServerProcessId(HANDLE(pipe.as_raw_handle()), &mut pid) }.is_err() {
        return false;
    }
    // SAFETY: query-only access; the handle is closed right after use.
    let Ok(process) = (unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }) else {
        return false;
    };
    let theirs = token_user(process);
    // SAFETY: handle returned by OpenProcess above, closed once.
    let _ = unsafe { CloseHandle(process) };
    // SAFETY: pseudo-handle of this process; it needs no closing.
    let ours = token_user(unsafe { GetCurrentProcess() });
    match (theirs, ours) {
        // SAFETY: both SIDs point into their live, aligned TOKEN_USER buffers.
        (Some(a), Some(b)) => unsafe { EqualSid(user_sid(&a), user_sid(&b)) }.is_ok(),
        _ => false,
    }
}

/// TOKEN_USER of a process. Held in u64s: the struct contains a pointer, so the
/// buffer must be pointer-aligned.
fn token_user(process: HANDLE) -> Option<Vec<u64>> {
    let mut token = HANDLE::default();
    // SAFETY: `process` is a valid process handle; `token` receives a new handle.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }.ok()?;
    let mut len = 0u32;
    // SAFETY: size query with no buffer; it fails with the needed length.
    let _ = unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) };
    let mut buf = vec![0u64; (len as usize).div_ceil(8)];
    // SAFETY: `buf` holds at least `len` bytes and is 8-byte aligned.
    let res = unsafe { GetTokenInformation(token, TokenUser, Some(buf.as_mut_ptr().cast()), len, &mut len) };
    // SAFETY: token handle opened above, closed once.
    let _ = unsafe { CloseHandle(token) };
    res.ok().map(|()| buf)
}

fn user_sid(buf: &[u64]) -> PSID {
    // SAFETY: `buf` was filled by GetTokenInformation(TokenUser) and is aligned.
    unsafe { (*buf.as_ptr().cast::<TOKEN_USER>()).User.Sid }
}

/// The server end is still there (a closed pipe makes the peek fail).
fn alive(pipe: &File) -> bool {
    let mut available = 0u32;
    // SAFETY: valid pipe handle owned by `pipe`; only the byte count is requested.
    unsafe {
        PeekNamedPipe(
            HANDLE(pipe.as_raw_handle()),
            None,
            0,
            None,
            Some(&mut available),
            None,
        )
    }
    .is_ok()
}

fn send(pipe: &mut File, payload: &serde_json::Value) -> io::Result<()> {
    pipe.write_all(&encode(OP_FRAME, payload))?;
    loop {
        let (op, body) = read_frame(pipe)?;
        match op {
            OP_CLOSE => {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "closed by Discord",
                ));
            }
            // Keep-alive from Discord: answer and wait for the actual response.
            OP_PING => {
                let ping: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
                pipe.write_all(&encode(OP_PONG, &ping))?;
            }
            _ => {
                if body.windows(13).any(|w| w == br#""evt":"ERROR""#) {
                    log::warn!(
                        "Discord rejected the activity: {}",
                        String::from_utf8_lossy(&body)
                    );
                }
                return Ok(());
            }
        }
    }
}

fn read_frame(pipe: &mut File) -> io::Result<(u32, Vec<u8>)> {
    let mut header = [0u8; 8];
    pipe.read_exact(&mut header)?;
    let (op, len) = decode_header(header)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "oversized Discord frame"))?;
    let mut body = vec![0u8; len as usize];
    pipe.read_exact(&mut body)?;
    Ok((op, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(title: &str) -> Track {
        Some(Activity {
            title: title.into(),
            artist: String::new(),
            album: String::new(),
            artwork: String::new(),
            start_ms: 0,
            end_ms: 0,
            track_url: String::new(),
            artist_url: String::new(),
            album_url: String::new(),
            paused: false,
            state_text: String::new(),
            button_label: String::new(),
        })
    }

    #[test]
    fn pipe_server_of_this_user_is_accepted() {
        use windows::Win32::{
            Storage::FileSystem::PIPE_ACCESS_DUPLEX,
            System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE},
        };
        use windows::core::HSTRING;
        let name = format!(r"\\.\pipe\ytm-test-{}", std::process::id());
        // SAFETY: plain pipe creation; the handle is closed at the end of the test.
        let server = unsafe {
            CreateNamedPipeW(&HSTRING::from(name.as_str()), PIPE_ACCESS_DUPLEX, PIPE_TYPE_BYTE, 1, 512, 512, 0, None)
        };
        assert!(!server.is_invalid());
        let client = OpenOptions::new().read(true).write(true).open(&name).unwrap();
        assert!(server_is_same_user(&client));
        drop(client);
        // SAFETY: handle created above.
        let _ = unsafe { CloseHandle(server) };
    }

    #[test]
    fn burst_settles_to_latest_state() {
        let (tx, rx) = mpsc::channel();
        for t in ["old", "new title, old timing", "new, complete"] {
            tx.send(Msg::Update(track(t))).unwrap();
        }
        match next_settled(&rx, None, None) {
            Event::Msg(Msg::Update(Some(a))) => assert_eq!(a.title, "new, complete"),
            _ => panic!("expected the latest update"),
        }
        assert!(matches!(
            next_settled(&rx, None, Some(Duration::from_millis(10))),
            Event::Tick
        ));
        tx.send(Msg::Stop).unwrap();
        assert!(matches!(
            next_settled(&rx, None, None),
            Event::Msg(Msg::Stop)
        ));
    }
}
