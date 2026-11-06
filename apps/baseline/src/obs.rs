//! OBS widget: a read-only HTTP server on 127.0.0.1 that serves `assets/obs.html`
//! (for an OBS "Browser" source) and the player state as Server-Sent Events.
//! Started only when enabled in Options; no thread or socket otherwise.
//!
//! Widget sockets are written by a dedicated thread, never by the caller of
//! `push` (the app's main thread): a local peer that connects and stops reading
//! can delay other widgets, not the app. At most `MAX_CLIENTS` widgets at once.

use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use ytm_core::PlayerState;

const PAGE: &str = include_str!("../../../assets/obs.html");
/// OBS needs one connection per Browser source; a few spare for reloads.
const MAX_CLIENTS: usize = 8;

struct Feed {
    state: PlayerState,
    at: Instant,
    clients: Vec<TcpStream>,
    /// Widgets the writer thread holds while it writes (still count toward the cap).
    writing: usize,
}

impl Feed {
    /// One SSE message. While playing, the position is advanced by the time
    /// since the last state, so a widget opened mid-track starts at the right place.
    fn event(&self) -> String {
        let mut s = self.state.clone();
        if s.playing {
            let elapsed = u32::try_from(self.at.elapsed().as_secs()).unwrap_or(u32::MAX);
            s.position = s.position.saturating_add(elapsed);
            if s.duration > 0 {
                s.position = s.position.min(s.duration);
            }
        }
        // Compact JSON has no newline, so it fits on one `data:` line.
        format!("data: {}\n\n", serde_json::to_string(&s).unwrap_or_default())
    }
}

pub struct Server {
    port: u16,
    stop: Arc<AtomicBool>,
    feed: Arc<Mutex<Feed>>,
    thread: Option<JoinHandle<()>>,
    /// Wakes the writer thread; dropping it ends that thread.
    wake: Sender<()>,
}

impl Server {
    /// Port 0 picks a free port (tests).
    pub fn start(port: u16, state: PlayerState) -> io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, port))?;
        let port = listener.local_addr()?.port();
        let stop = Arc::new(AtomicBool::new(false));
        let feed = Arc::new(Mutex::new(Feed {
            state,
            at: Instant::now(),
            clients: Vec::new(),
            writing: 0,
        }));
        let (s, f) = (stop.clone(), feed.clone());
        let thread = std::thread::Builder::new()
            .name("obs-widget".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    if s.load(Ordering::Relaxed) {
                        break;
                    }
                    // ponytail: one request at a time on this thread (2 s read
                    // timeout); fine for a local widget, a pool if it ever is not.
                    if let Ok(c) = conn
                        && let Err(e) = handle(c, port, &f)
                    {
                        log::debug!("OBS widget request: {e}");
                    }
                }
            })?;
        let (wake, rx) = mpsc::channel::<()>();
        let f = feed.clone();
        std::thread::Builder::new()
            .name("obs-widget-push".into())
            .spawn(move || {
                while rx.recv().is_ok() {
                    while rx.try_recv().is_ok() {} // several updates: send the latest once
                    // Write outside the lock so push() and new widgets never wait on a slow one.
                    let (msg, mut clients) = match f.lock() {
                        Ok(mut f) => {
                            let clients = std::mem::take(&mut f.clients);
                            f.writing = clients.len();
                            (f.event(), clients)
                        }
                        Err(_) => return,
                    };
                    clients.retain_mut(|c| c.write_all(msg.as_bytes()).is_ok());
                    if let Ok(mut f) = f.lock() {
                        f.writing = 0;
                        f.clients.extend(clients);
                    }
                }
            })?;
        log::info!("OBS widget on http://127.0.0.1:{port}/");
        Ok(Self {
            port,
            stop,
            feed,
            thread: Some(thread),
            wake,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Records the new state and lets the writer thread send it; never blocks on sockets.
    pub fn push(&self, state: &PlayerState) {
        if let Ok(mut f) = self.feed.lock() {
            f.state = state.clone();
            f.at = Instant::now();
        }
        let _ = self.wake.send(());
    }
}

impl Drop for Server {
    /// Stops the thread and frees the port before returning, so the same port
    /// can be bound again right away.
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Wakes the blocking accept().
        let _ = TcpStream::connect((Ipv4Addr::LOCALHOST, self.port));
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut f) = self.feed.lock() {
            for c in f.clients.drain(..) {
                let _ = c.shutdown(Shutdown::Both);
            }
        }
    }
}

fn handle(mut c: TcpStream, port: u16, feed: &Mutex<Feed>) -> io::Result<()> {
    c.set_read_timeout(Some(Duration::from_secs(2)))?;
    // A widget that stops reading must not stall the app for long.
    c.set_write_timeout(Some(Duration::from_secs(1)))?;
    let mut reader = BufReader::new(c.try_clone()?.take(8 * 1024));
    let mut request = String::new();
    reader.read_line(&mut request)?;
    let mut host = String::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((k, v)) = line.split_once(':')
            && k.eq_ignore_ascii_case("host")
        {
            host = v.trim().to_ascii_lowercase();
        }
    }
    // Host check: a web page that rebinds its own domain to 127.0.0.1 sends its
    // domain here, not ours, and is refused.
    if host != format!("127.0.0.1:{port}") && host != format!("localhost:{port}") {
        return respond(&mut c, "403 Forbidden", "text/plain", "forbidden");
    }
    let mut parts = request.split_whitespace();
    let (method, target) = (parts.next(), parts.next().unwrap_or(""));
    if method != Some("GET") {
        return respond(&mut c, "405 Method Not Allowed", "text/plain", "GET only");
    }
    match target.split('?').next() {
        Some("/") => respond(&mut c, "200 OK", "text/html; charset=utf-8", PAGE),
        Some("/events") => {
            let first = match feed.lock().map(|mut f| {
                // Widgets that went away (OBS reloaded the source) free their slot
                // now, not at the next push: nothing is pushed while paused.
                f.clients.retain(|c| !closed(c));
                f
            }) {
                Ok(f) if f.clients.len() + f.writing < MAX_CLIENTS => f.event(),
                Ok(_) => {
                    return respond(&mut c, "503 Service Unavailable", "text/plain", "too many widgets");
                }
                Err(_) => return Ok(()),
            };
            // Written without the lock held: a slow peer cannot hold up push().
            c.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\
                  Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\n\r\n",
            )?;
            c.write_all(first.as_bytes())?;
            if let Ok(mut f) = feed.lock() {
                f.clients.push(c);
            }
            Ok(())
        }
        _ => respond(&mut c, "404 Not Found", "text/plain", "not found"),
    }
}

/// The peer closed the connection (a widget never sends anything after its request).
fn closed(c: &TcpStream) -> bool {
    if c.set_nonblocking(true).is_err() {
        return true;
    }
    let gone = match c.peek(&mut [0u8; 1]) {
        Ok(0) => true,
        Ok(_) => false,
        Err(e) => e.kind() != io::ErrorKind::WouldBlock,
    };
    let _ = c.set_nonblocking(false);
    gone
}

fn respond(c: &mut TcpStream, status: &str, kind: &str, body: &str) -> io::Result<()> {
    write!(
        c,
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Cache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(port: u16, path: &str, host: &str) -> (TcpStream, BufReader<TcpStream>) {
        let mut c = TcpStream::connect((Ipv4Addr::LOCALHOST, port)).unwrap();
        c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        write!(c, "GET {path} HTTP/1.1\r\nHost: {host}\r\n\r\n").unwrap();
        let r = BufReader::new(c.try_clone().unwrap());
        (c, r)
    }

    fn status(r: &mut BufReader<TcpStream>) -> String {
        let mut line = String::new();
        r.read_line(&mut line).unwrap();
        line
    }

    /// Next `data:` line of an event stream.
    fn data(r: &mut BufReader<TcpStream>) -> String {
        loop {
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            if let Some(d) = line.strip_prefix("data: ") {
                return d.trim().to_string();
            }
        }
    }

    #[test]
    fn serves_page_and_pushes_state_to_local_hosts_only() {
        let first = PlayerState {
            title: "One".into(),
            ..PlayerState::default()
        };
        let server = Server::start(0, first).unwrap();
        let port = server.port();
        let host = format!("127.0.0.1:{port}");

        let (_c, mut r) = get(port, "/", &host);
        assert!(status(&mut r).contains("200"));
        let (_c, mut r) = get(port, "/", &format!("evil.example:{port}"));
        assert!(status(&mut r).contains("403"));
        let (_c, mut r) = get(port, "/nope", &host);
        assert!(status(&mut r).contains("404"));

        let (_c, mut r) = get(port, "/events", &format!("localhost:{port}"));
        assert!(status(&mut r).contains("200"));
        assert!(data(&mut r).contains("\"title\":\"One\""));
        server.push(&PlayerState {
            title: "Two".into(),
            playing: true,
            ..PlayerState::default()
        });
        assert!(data(&mut r).contains("\"title\":\"Two\""));

        // A full house is refused instead of growing without bound.
        let mut held = Vec::new();
        for _ in 1..MAX_CLIENTS {
            let (c, mut r) = get(port, "/events", &host);
            assert!(status(&mut r).contains("200"));
            held.push(c);
        }
        let (_c, mut r) = get(port, "/events", &host);
        assert!(status(&mut r).contains("503"));
        // None of these widgets reads: push() must still return at once.
        let t = Instant::now();
        for _ in 0..200 {
            server.push(&PlayerState::default());
        }
        assert!(t.elapsed() < Duration::from_millis(500));
        // Closed widgets free their slots without waiting for a push.
        drop(held);
        drop(_c);
        std::thread::sleep(Duration::from_millis(100));
        let (_c, mut r) = get(port, "/events", &host);
        assert!(status(&mut r).contains("200"));

        drop(server); // must return: the thread is stopped and joined
        assert!(TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
    }
}
