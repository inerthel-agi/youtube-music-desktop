//! YouTube Music desktop (MVP base): Tauri 2 + one WebView2 window on
//! music.youtube.com, a tray icon, close-to-tray, a lazily created mini-player
//! and settings saved locally.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod discord;
mod features;
mod obs;
mod options;
mod tray;
#[cfg(windows)]
mod win;

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use tauri::{
    AppHandle, LogicalPosition, Manager, RunEvent, State, Webview, WebviewBuilder, WebviewUrl,
    WebviewWindowBuilder, Window, WindowEvent, webview::PageLoadEvent,
};
use ytm_core::{Command, PlayerState, Settings, WindowBounds, parse_bridge_msg};

/// Main window (frameless) and the music webview filling it. Options is a second
/// child webview laid over the same window while open.
pub(crate) const MAIN: &str = "main";
const MUSIC: &str = "ytm";
const MINI: &str = "mini";
const BRIDGE_JS: &str = include_str!("../../../assets/bridge.js");
const TITLEBAR_JS: &str = include_str!("../../../assets/titlebar.js");
const AUDIOFX_JS: &str = include_str!("../../../assets/audiofx.js");
const LEAN_JS: &str = include_str!("../../../assets/lean.js");
/// Command-line flag set in the Windows "Run" entry.
pub(crate) const AUTOSTART_FLAG: &str = "--autostart";
/// YouTube Music's page background, used for every native surface.
pub(crate) const BG: tauri::webview::Color = tauri::webview::Color(3, 3, 3, 255);

static START: OnceLock<Instant> = OnceLock::new();

/// Set on the process the app restarts itself into: times of the restarts that
/// followed a WebView2 crash (`ytm_core::Retries::encode`, may be empty). Its
/// presence also tells the new process to wait for the old one to exit.
const RESTART_ENV: &str = "YTM_RESTARTS";
/// Crash restarts allowed within the window, then the app quits instead of looping.
const CRASH_RESTARTS: (usize, u64) = (3, 600);
static RESTART_HISTORY: OnceLock<String> = OnceLock::new();

pub(crate) fn unix_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Restarts the app. After a crash the restart counts against `CRASH_RESTARTS`;
/// past it the app shows a notification and quits rather than loop.
pub(crate) fn restart(app: &AppHandle, after_crash: bool) {
    let (max, window) = CRASH_RESTARTS;
    let mut history = ytm_core::Retries::decode(
        RESTART_HISTORY.get().map_or("", String::as_str),
        max,
        window,
    );
    if after_crash && !history.allow(unix_secs()) {
        log::error!("WebView2 crashed {max} times in {window} s; quitting instead of restarting");
        #[cfg(windows)]
        {
            let (title, body) = if french_ui(app) {
                (
                    "YouTube Music s'est arrêté",
                    "WebView2 plante à répétition. Relancez l'application ou réparez Microsoft Edge WebView2.",
                )
            } else {
                (
                    "YouTube Music stopped",
                    "WebView2 keeps crashing. Start the app again or repair Microsoft Edge WebView2.",
                )
            };
            let _ = win::notify(title, body);
        }
        return quit(app);
    }
    save_settings(app);
    // SAFETY: on Windows `set_var` is safe from any thread (std docs). The child
    // inherits the variable; `main` reads and removes it.
    unsafe { std::env::set_var(RESTART_ENV, history.encode()) };
    app.restart();
}

/// Browser permissions of the music page. YouTube Music plays and Google signs
/// in without any of the denied ones; others (autoplay, DRM key systems) keep
/// WebView2's default.
fn music_permission(kind: tauri::webview::PermissionKind) -> tauri::webview::PermissionResponse {
    use tauri::webview::{PermissionKind as K, PermissionResponse as R};
    match kind {
        K::Microphone
        | K::Camera
        | K::Geolocation
        | K::Notifications
        | K::ClipboardRead
        | K::DisplayCapture
        | K::Midi
        | K::Sensors
        | K::LocalFonts
        | K::WindowManagement
        | K::AutomaticDownloads
        | K::FileSystemAccess => {
            log::info!("permission {kind} denied to the music page");
            R::Deny
        }
        _ => R::Default,
    }
}

fn ms() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

pub(crate) struct Shared {
    settings: Mutex<Settings>,
    settings_path: PathBuf,
    player: Mutex<Player>,
    /// Settings at launch: restart-only options are compared against these.
    launch: Settings,
    /// Same WebView2 arguments for every window (one shared browser process).
    browser_args: String,
    presence: Mutex<Option<discord::Presence>>,
    /// Current WebView2 memory target, so resize storms make one COM call at most.
    memory_low: std::sync::atomic::AtomicBool,
    /// OBS widget server, running while enabled in Options.
    obs: Mutex<Option<obs::Server>>,
}

struct Player {
    state: PlayerState,
    first_play_logged: bool,
    shell_reloaded: bool,
    page_errors: ytm_core::Retries,
}

impl Default for Player {
    fn default() -> Self {
        Self {
            state: PlayerState::default(),
            first_play_logged: false,
            shell_reloaded: false,
            page_errors: ytm_core::Retries::new(5, 60),
        }
    }
}

impl Player {
    fn take_shell_recovery(&mut self, state: &PlayerState, visible: bool) -> bool {
        let recover = visible
            && !self.shell_reloaded
            && (!state.title.is_empty() || !state.track_url.is_empty() || state.duration > 0)
            && state
                .missing
                .iter()
                .any(|m| matches!(m.as_str(), "nav-bar" | "player-bar"));
        self.shell_reloaded |= recover;
        recover
    }
}

/// Only fixed categories reach the log, even if the remote page forges a report.
fn page_error_label(raw: &str) -> Option<(&str, &str)> {
    if raw.len() > 64 {
        return None;
    }
    let (kind, name) = raw.split_once(':')?;
    if !matches!(kind, "error" | "unhandledrejection") {
        return None;
    }
    let name = match name {
        "Error" | "TypeError" | "ReferenceError" | "SyntaxError" | "RangeError" | "EvalError"
        | "URIError" | "DOMException" | "NotSupportedError" | "SecurityError" | "AbortError"
        | "TrustedTypes" => name,
        _ => "Other",
    };
    Some((kind, name))
}

fn shell_recovery_url(current: &tauri::Url, track: &str) -> Option<tauri::Url> {
    if current.scheme() != "https" || current.host_str() != Some("music.youtube.com") {
        return None; // Do not interrupt a sign-in redirect.
    }
    if track.is_empty() {
        return Some(current.clone());
    }
    let target: tauri::Url = track.parse().ok()?;
    if target.origin() != current.origin() {
        return None;
    }
    // Keep the queue parameters when this URL already identifies the playing track.
    if current.path() == "/watch"
        && current.query_pairs().find(|(k, _)| k == "v")
            == target.query_pairs().find(|(k, _)| k == "v")
    {
        Some(current.clone())
    } else {
        Some(target)
    }
}

pub(crate) fn shared(app: &AppHandle) -> State<'_, Shared> {
    app.state::<Shared>()
}

pub(crate) fn main_window(app: &AppHandle) -> Option<Window> {
    app.get_window(MAIN)
}

pub(crate) fn music(app: &AppHandle) -> Option<Webview> {
    app.get_webview(MUSIC)
}

#[tauri::command]
async fn player_event(
    app: AppHandle,
    msg: String,
    error: Option<String>,
    shell: Option<[u32; 4]>,
) -> Result<(), String> {
    if let Some(error) = error {
        let (kind, name) = page_error_label(&error).ok_or("invalid page error")?;
        let shared = shared(&app);
        let mut p = shared.player.lock().map_err(|_| "player lock poisoned")?;
        if p.page_errors.allow(unix_secs()) {
            log::warn!("YouTube Music page error: {kind} ({name})");
        }
        return Ok(());
    }
    let state = parse_bridge_msg(&msg).map_err(|e| {
        log::warn!("invalid player event message");
        e.to_string()
    })?;
    let visible = main_window(&app)
        .is_some_and(|w| w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(true));
    let (prev, recover) = {
        let shared = shared(&app);
        let mut p = shared.player.lock().map_err(|_| "player lock poisoned")?;
        if state.playing && !p.first_play_logged {
            p.first_play_logged = true;
            log::info!("timing: first playback at {} ms", ms());
        }
        let recover = p.take_shell_recovery(&state, visible);
        if p.state == state && !recover {
            return Ok(());
        }
        (std::mem::replace(&mut p.state, state.clone()), recover)
    };
    if state.missing != prev.missing && !state.missing.is_empty() {
        log::warn!(
            "YouTube Music page changed: {} not found; some features may stop working",
            state.missing.join(", ")
        );
        if let Some(shell) = shell {
            let [width, height, nav, player] = shell.map(|n| n.min(100_000));
            log::warn!(
                "shell diagnostics: viewport {width}x{height}, nav children {nav}, player children {player}, window visible {visible}"
            );
        }
    }
    if recover {
        log::warn!("YouTube Music shell empty; attempting one reload for this app session");
        if let Some(w) = music(&app) {
            let target = w
                .url()
                .ok()
                .and_then(|url| shell_recovery_url(&url, &state.track_url));
            if target.is_none_or(|url| {
                w.eval(format!(
                    "window.__ytmRecover({});",
                    serde_json::json!(url.as_str())
                ))
                .is_err()
            }) {
                log::warn!("YouTube Music shell recovery unavailable or failed; no retry");
            }
        }
    }
    features::on_state(&app, &prev, &state);
    tray::set_tooltip(&app, state.track_label());
    if let Some(w) = main_window(&app) {
        // Taskbar button and thumbnail show the current track.
        let title = state
            .track_label()
            .unwrap_or_else(|| "YouTube Music".into());
        let _ = w.set_title(&title);
    }
    #[cfg(windows)]
    {
        let s = state.clone();
        let _ = app.run_on_main_thread(move || {
            win::set_thumb_playing(s.playing);
            win::update_media_session(&s);
        });
    }
    push_to_mini(&app, &state);
    if let Ok(obs) = shared(&app).obs.lock()
        && let Some(server) = obs.as_ref()
    {
        server.push(&state);
    }
    update_presence(&app, &state);
    Ok(())
}

/// Async for the same reason as `window_control` (keeps the main thread free).
#[tauri::command]
async fn mini_command(app: AppHandle, cmd: String, value: Option<u32>) -> Result<(), String> {
    match cmd.as_str() {
        "ready" => {
            let state = shared(&app)
                .player
                .lock()
                .map_err(|_| "player lock poisoned")?
                .state
                .clone();
            push_to_mini(&app, &state);
        }
        "play_pause" => send(&app, Command::PlayPause),
        "next" => send(&app, Command::Next),
        "previous" => send(&app, Command::Previous),
        "volume" => send(
            &app,
            Command::SetVolume(value.unwrap_or(100).min(100) as u8),
        ),
        "seek" => send(&app, Command::Seek(value.unwrap_or(0))),
        "like" => send(&app, Command::Like),
        "dislike" => send(&app, Command::Dislike),
        "shuffle" => send(&app, Command::Shuffle),
        "repeat" => send(&app, Command::Repeat),
        "show_main" => show_main(&app),
        "close_mini" => close_mini(&app),
        other => return Err(format!("unknown command {other:?}")),
    }
    Ok(())
}

/// Caption buttons drawn by `assets/titlebar.js` in the frameless main window.
/// Async on purpose: sync commands run on the main thread, and creating a window
/// (Options) from there deadlocks the event loop on Windows.
#[tauri::command]
async fn window_control(window: Window, action: String) -> Result<(), String> {
    if window.label() != MAIN {
        return Err("not the main window".into());
    }
    let res = match action.as_str() {
        "minimize" => window.minimize(),
        "toggle_maximize" => {
            if window.is_maximized().unwrap_or(false) {
                window.unmaximize()
            } else {
                window.maximize()
            }
        }
        // Goes through CloseRequested: hides to the tray or quits per settings.
        "close" => window.close(),
        "options" => {
            options::open(window.app_handle());
            Ok(())
        }
        "close_options" => {
            options::close(window.app_handle());
            Ok(())
        }
        "compact" => {
            features::toggle_compact(window.app_handle());
            Ok(())
        }
        other => return Err(format!("unknown action {other:?}")),
    };
    res.map_err(|e| e.to_string())
}

/// Sends a player command to the YouTube Music page.
pub(crate) fn send(app: &AppHandle, cmd: Command) {
    if let Some(w) = music(app)
        && let Err(e) = w.eval(cmd.to_js())
    {
        log::warn!("eval {cmd:?} failed: {e}");
    }
}

pub(crate) fn show_main(app: &AppHandle) {
    let Some(w) = main_window(app) else {
        return;
    };
    let _ = w.show();
    let _ = w.unminimize();
    let _ = w.set_focus();
    #[cfg(windows)]
    set_memory_low(app, false);
}

pub(crate) fn hide_main(app: &AppHandle) {
    let Some(w) = main_window(app) else {
        return;
    };
    remember_bounds(app);
    save_settings(app); // the process may be ended while hidden
    let _ = w.hide();
    #[cfg(windows)]
    set_memory_low(app, true);
}

/// Low WebView2 memory target while the window is hidden or minimized.
#[cfg(windows)]
fn set_memory_low(app: &AppHandle, low: bool) {
    let flag = &shared(app).memory_low;
    if flag.swap(low, std::sync::atomic::Ordering::Relaxed) != low
        && let Some(w) = music(app)
    {
        win::set_low_memory(&w, low);
        // A child WebView2 can keep document.hidden=false while its window is minimized.
        if w.eval(format!(
            "window.__ytmSetVisible && window.__ytmSetVisible({});",
            !low
        ))
        .is_err()
        {
            log::warn!("page visibility not applied");
        }
    }
}

pub(crate) fn toggle_main(app: &AppHandle) {
    let visible = main_window(app)
        .and_then(|w| Some(w.is_visible().ok()? && !w.is_minimized().ok()?))
        .unwrap_or(false);
    if visible {
        hide_main(app)
    } else {
        show_main(app)
    }
}

/// The mini-player is a second, small local page created on demand and
/// destroyed on close, so it costs nothing while unused.
pub(crate) fn toggle_mini(app: &AppHandle) {
    if app.get_webview_window(MINI).is_some() {
        return close_mini(app);
    }
    let on_top = shared(app)
        .settings
        .lock()
        .map(|s| s.mini_on_top)
        .unwrap_or(true);
    let res = WebviewWindowBuilder::new(app, MINI, WebviewUrl::App("mini.html".into()))
        .title("YouTube Music mini player")
        .inner_size(480.0, 112.0)
        .resizable(false)
        .maximizable(false)
        .decorations(false)
        .always_on_top(on_top)
        .skip_taskbar(true)
        .theme(Some(tauri::Theme::Dark))
        .background_color(BG)
        .additional_browser_args(&shared(app).browser_args)
        .build();
    if let Err(e) = res {
        log::error!("cannot open mini-player: {e}");
    }
}

fn close_mini(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(MINI) {
        let _ = w.destroy();
    }
}

fn push_to_mini(app: &AppHandle, state: &PlayerState) {
    let Some(mini) = app.get_webview_window(MINI) else {
        return;
    };
    // JSON is a valid JS literal; the page only assigns it with textContent.
    match serde_json::to_string(state) {
        Ok(json) => {
            if let Err(e) = mini.eval(format!("window.render && window.render({json})")) {
                log::warn!("mini-player update failed: {e}");
            }
        }
        Err(e) => log::error!("cannot serialize player state: {e}"),
    }
}

/// Stores the main window's normal bounds (kept as-is while maximized).
fn remember_bounds(app: &AppHandle) {
    let Some(w) = main_window(app) else {
        return;
    };
    if !w.is_visible().unwrap_or(false) || w.is_minimized().unwrap_or(true) {
        return;
    }
    let shared = shared(app);
    let Ok(mut settings) = shared.settings.lock() else {
        return;
    };
    if w.is_maximized().unwrap_or(false) {
        if let Some(b) = settings.window.as_mut() {
            b.maximized = true;
        }
        return;
    }
    let (Ok(scale), Ok(pos), Ok(size)) = (w.scale_factor(), w.outer_position(), w.inner_size())
    else {
        return;
    };
    let (pos, size) = (pos.to_logical::<f64>(scale), size.to_logical::<f64>(scale));
    settings.window = WindowBounds {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
        maximized: false,
    }
    .sanitized();
}

pub(crate) fn save_settings(app: &AppHandle) {
    let shared = shared(app);
    let Ok(settings) = shared.settings.lock() else {
        return;
    };
    if let Err(e) = settings.save(&shared.settings_path) {
        log::error!("cannot save settings: {e}");
    }
}

pub(crate) fn set_close_to_tray(app: &AppHandle, value: bool) {
    if let Ok(mut s) = shared(app).settings.lock() {
        s.close_to_tray = value;
    }
    save_settings(app);
}

pub(crate) fn close_to_tray(app: &AppHandle) -> bool {
    shared(app)
        .settings
        .lock()
        .map(|s| s.close_to_tray)
        .unwrap_or(true)
}

pub(crate) fn quit(app: &AppHandle) {
    remember_bounds(app);
    save_settings(app);
    app.exit(0);
}

/// Applies the page-side settings (audio processing, auto-pause on device change).
/// Called on every page load and when they change.
pub(crate) fn apply_page_settings(app: &AppHandle) {
    let js = match shared(app).settings.lock() {
        Ok(s) => features::page_settings_js(&s),
        Err(_) => return,
    };
    let visible = !shared(app)
        .memory_low
        .load(std::sync::atomic::Ordering::Relaxed);
    let js = format!("window.__ytmSetVisible && window.__ytmSetVisible({visible}); {js}");
    if let Some(w) = music(app)
        && let Err(e) = w.eval(js)
    {
        log::warn!("page settings not applied: {e}");
    }
}

pub(crate) fn set_discord(app: &AppHandle, on: bool) {
    let shared = shared(app);
    let Ok(mut presence) = shared.presence.lock() else {
        return;
    };
    // Dropping the worker clears the activity and closes the pipe.
    *presence = on.then(discord::Presence::start);
    drop(presence);
    if on {
        let state = shared
            .player
            .lock()
            .map(|p| p.state.clone())
            .unwrap_or_default();
        update_presence(app, &state);
    }
}

/// Starts the OBS widget server on `port`, or stops it with `None`.
pub(crate) fn set_obs(app: &AppHandle, port: Option<u16>) {
    let shared = shared(app);
    let Ok(mut obs) = shared.obs.lock() else {
        return;
    };
    *obs = None; // stops the previous server and frees its port first
    let Some(port) = port else {
        return;
    };
    let state = shared
        .player
        .lock()
        .map(|p| p.state.clone())
        .unwrap_or_default();
    match obs::Server::start(port, state) {
        Ok(server) => *obs = Some(server),
        Err(e) => log::warn!("OBS widget: cannot listen on 127.0.0.1:{port}: {e}"),
    }
}

/// Port of the running OBS widget server.
pub(crate) fn obs_port(app: &AppHandle) -> Option<u16> {
    shared(app).obs.lock().ok()?.as_ref().map(obs::Server::port)
}

/// App labels in French: setting "fr", or "system" on a French Windows.
pub(crate) fn french_ui(app: &AppHandle) -> bool {
    let lang = shared(app)
        .settings
        .lock()
        .map(|s| s.language.clone())
        .unwrap_or_default();
    match lang.as_str() {
        "fr" => true,
        #[cfg(windows)]
        "system" => win::ui_lang_is_french(),
        _ => false,
    }
}

fn update_presence(app: &AppHandle, state: &PlayerState) {
    let shared = shared(app);
    let Ok(presence) = shared.presence.lock() else {
        return;
    };
    if let Some(p) = presence.as_ref() {
        // Kept while paused (pause icon, no progress bar); cleared during ads / when idle.
        let track = (!state.ad && !state.title.is_empty()).then(|| {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_millis() as u64);
            let start_ms = now_ms.saturating_sub(u64::from(state.position) * 1000);
            let end_ms = if state.duration > 0 {
                start_ms + u64::from(state.duration) * 1000
            } else {
                0
            };
            ytm_core::discord::Activity {
                title: state.title.clone(),
                artist: state.artist.clone(),
                album: state.album.clone(),
                artwork: state.artwork.clone(),
                start_ms,
                end_ms,
                track_url: state.track_url.clone(),
                artist_url: state.artist_url.clone(),
                album_url: state.album_url.clone(),
                paused: !state.playing,
                state_text: match (state.playing, french_ui(app)) {
                    (true, true) => "Lecture",
                    (true, false) => "Playing",
                    (false, true) => "En pause",
                    (false, false) => "Paused",
                }
                .into(),
                button_label: if french_ui(app) {
                    "Écouter sur YouTube Music"
                } else {
                    "Listen on YouTube Music"
                }
                .into(),
            }
        });
        p.update(track);
    }
}

/// Saved position only if its top-left corner is on a connected monitor.
fn on_screen(app: &AppHandle, b: &WindowBounds) -> bool {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .any(|m| {
            let (p, s, f) = (m.position(), m.size(), m.scale_factor());
            let (x, y) = (b.x * f, b.y * f);
            x >= f64::from(p.x) - 8.0
                && y >= f64::from(p.y) - 8.0
                && x < f64::from(p.x) + f64::from(s.width) - 100.0
                && y < f64::from(p.y) + f64::from(s.height) - 100.0
        })
}

fn build_main(
    app: &AppHandle,
    settings: &Settings,
    browser_args: &str,
    start_hidden: bool,
) -> Result<Window, Box<dyn std::error::Error>> {
    // Start page: YTM_START_URL (benchmarks), else the last track when "resume"
    // is on, else the home page. All are validated to stay on music.youtube.com.
    let resume = features::resume_script(settings);
    let start = std::env::var("YTM_START_URL").ok().or_else(|| {
        resume
            .as_ref()
            .and(settings.last_track.as_ref())
            .map(|t| t.url.clone())
    });
    let url = ytm_core::start_url(start.as_deref()).parse()?;
    let bounds = settings.window;
    let size = bounds.map_or((1200.0, 800.0), |b| (b.width, b.height));
    let mut builder = tauri::WindowBuilder::new(app, MAIN)
        .title("YouTube Music")
        .inner_size(size.0, size.1)
        .min_inner_size(ytm_core::MIN_WIDTH, ytm_core::MIN_HEIGHT)
        .theme(Some(tauri::Theme::Dark))
        // Frameless: the page (and its gradient) reaches the top edge; titlebar.js
        // draws the caption buttons. Shadow keeps the native border and resize edges.
        .decorations(false)
        .shadow(true)
        .visible(false); // shown once the frame is styled: no flash
    if let Some(b) = bounds
        && on_screen(app, &b)
    {
        builder = builder.position(b.x, b.y);
    }
    let window = builder.build()?;
    let mut music = WebviewBuilder::new(MUSIC, WebviewUrl::External(url)).background_color(BG);
    if let Some(js) = resume {
        music = music.initialization_script(&js); // read by bridge.js
    }
    let music = music
        .initialization_script(BRIDGE_JS)
        .initialization_script(TITLEBAR_JS)
        .initialization_script(AUDIOFX_JS)
        .initialization_script(LEAN_JS)
        .additional_browser_args(browser_args)
        // Resized with the window in on_window_event (WindowEvent::Resized).
        .on_navigation(|url| {
            let ok = ytm_core::navigation_allowed(url.scheme(), url.host_str().unwrap_or(""));
            if !ok {
                log::warn!(
                    "navigation blocked: {}://{}",
                    url.scheme(),
                    url.host_str().unwrap_or("")
                );
            }
            ok
        })
        .on_permission_request(|_, kind| music_permission(kind))
        // No file downloads from the music page: YouTube Music offers none, and
        // with SmartScreen off (Tauri default) nothing would vet them.
        .on_download(|_, e| match e {
            tauri::webview::DownloadEvent::Requested { url, .. } => {
                log::warn!("download blocked ({})", url.host_str().unwrap_or(""));
                false
            }
            _ => true,
        })
        .on_page_load(|w, p| {
            if p.event() == PageLoadEvent::Finished {
                // Host only: full URLs would put the listening history in the log.
                let host = p.url().host_str().unwrap_or("").to_owned();
                log::info!("timing: page loaded at {} ms ({host})", ms());
                if host == "music.youtube.com" {
                    apply_page_settings(w.app_handle());
                }
            }
        });
    let music = window.add_child(
        music,
        LogicalPosition::new(0.0, 0.0),
        tauri::LogicalSize::new(size.0, size.1),
    )?;
    #[cfg(windows)]
    {
        win::style_frame(&window);
        win::watch_process_failures(&music);
        if let Err(e) = win::install_thumbbar(&window) {
            log::warn!("taskbar thumbnail buttons unavailable: {e}");
        }
        win::register_hotkeys(&settings.hotkeys);
    }
    if start_hidden {
        // Started with Windows: stay in the tray until the icon is clicked.
        #[cfg(windows)]
        set_memory_low(app, true);
        return Ok(window);
    }
    if bounds.is_some_and(|b| b.maximized) {
        window.maximize()?;
    }
    window.show()?;
    Ok(window)
}

fn log_level() -> log::LevelFilter {
    std::env::var("YTM_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(log::LevelFilter::Info)
}

fn main() {
    START.get_or_init(Instant::now);
    let restarted = std::env::var(RESTART_ENV).ok();
    // SAFETY: no other thread exists yet. Removed so processes the app starts
    // (browser for links) do not inherit it.
    unsafe { std::env::remove_var(RESTART_ENV) };
    #[cfg(windows)]
    {
        // A second launch brings back the running window instead of opening another one.
        if win::another_instance_running(
            std::env::args().any(|a| a == AUTOSTART_FLAG),
            restarted.is_some(),
        ) {
            return;
        }
        win::set_app_id(); // before any window: taskbar and media session identity
    }
    if let Some(h) = restarted {
        let _ = RESTART_HISTORY.set(h);
    }
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log_level())
                // Default is 40 KB + KeepOne, which silently discards history.
                .max_file_size(2_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(2))
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            player_event,
            mini_command,
            window_control,
            options::get_settings,
            options::update_settings,
            options::audio_preset,
            options::restart_app,
            options::open_url,
            options::eq_preset,
            features::get_sleep_timer,
            features::set_sleep_timer
        ])
        .on_window_event(|w, e| {
            if w.label() != MAIN {
                return;
            }
            let app = w.app_handle();
            match e {
                WindowEvent::CloseRequested { api, .. } => {
                    if close_to_tray(app) {
                        api.prevent_close();
                        hide_main(app);
                    } else {
                        quit(app);
                    }
                }
                WindowEvent::Resized(size) => {
                    let minimized = w.is_minimized().unwrap_or(false);
                    // Minimize / restore: same memory trim as hiding to the tray.
                    #[cfg(windows)]
                    set_memory_low(app, minimized);
                    // The child webviews fill the window. Not Tauri's auto_resize: it also
                    // applies the minimized client area (about 144x19), and YouTube Music
                    // then switches to its mobile layout and never fully comes back
                    // (empty top bar and player bar after restore).
                    if !minimized {
                        for v in w.webviews() {
                            if let Err(e) = v.set_size(*size) {
                                log::warn!("cannot resize webview {}: {e}", v.label());
                            }
                        }
                    }
                }
                _ => {}
            }
        })
        .setup(|app| {
            log::info!("timing: setup at {} ms", ms());
            let settings_path = app.path().app_config_dir()?.join("settings.json");
            let (settings, err) = Settings::load(&settings_path);
            if let Some(err) = err {
                log::warn!("{err}; using defaults");
            }
            let saved = settings.clone();
            // YTM_BROWSER_ARGS replaces the computed WebView2 arguments (benchmarks only).
            let browser_args =
                std::env::var("YTM_BROWSER_ARGS").unwrap_or_else(|_| settings.browser_args());
            let autostarted = std::env::args().any(|a| a == AUTOSTART_FLAG);
            // Managed before any window exists: events may need it immediately.
            app.manage(Shared {
                settings: Mutex::new(settings),
                settings_path,
                player: Mutex::new(Player::default()),
                launch: saved.clone(),
                browser_args: browser_args.clone(),
                presence: Mutex::new(None),
                memory_low: std::sync::atomic::AtomicBool::new(false),
                obs: Mutex::new(None),
            });
            app.manage(Mutex::new(features::Sleep::default()));
            app.manage(Mutex::new(features::Compact::default()));
            let handle = app.handle();
            let window = build_main(
                handle,
                &saved,
                &browser_args,
                autostarted && saved.start_hidden,
            )?;
            log::info!("timing: window ready at {} ms", ms());
            #[cfg(windows)]
            {
                if let Err(e) = win::install_media_session(&window) {
                    log::warn!("media session unavailable: {e}");
                }
                if let Err(e) = win::ensure_start_menu_shortcut() {
                    log::warn!("Start menu shortcut not created: {e}");
                }
            }
            tray::create(handle, &saved)?;
            if saved.discord {
                set_discord(handle, true);
            }
            if saved.obs_widget {
                set_obs(handle, Some(saved.obs_port));
            }
            // Keep the Windows "Run" entry on this executable if it was moved or renamed.
            #[cfg(windows)]
            if saved.launch_at_startup
                && let Err(e) = win::set_autostart(true)
            {
                log::warn!("start with Windows not refreshed: {e}");
            }
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri app");

    app.run(|app, event| {
        if let RunEvent::Exit = event {
            // Also reached when Windows shuts down or signs out (tao exits the
            // process right after): keep the last track and window bounds.
            remember_bounds(app);
            save_settings(app);
            log::info!("exit after {} ms", ms());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{Player, music_permission, page_error_label, shell_recovery_url};
    use tauri::webview::{PermissionKind as K, PermissionResponse as R};
    use ytm_core::PlayerState;

    #[test]
    fn empty_shell_recovers_only_once_across_page_loads_and_tracks() {
        let mut player = Player::default();
        let mut state = PlayerState {
            missing: vec!["nav-bar".into()],
            ..PlayerState::default()
        };
        assert!(!player.take_shell_recovery(&state, true)); // No track loaded.
        state.duration = 180; // Media Session metadata can also be missing.
        state.missing = vec!["like".into()];
        assert!(!player.take_shell_recovery(&state, true));
        for part in ["nav-bar", "player-bar"] {
            state.missing = vec![part.into()];
            let mut fresh = Player::default();
            assert!(!fresh.take_shell_recovery(&state, false)); // Hidden/minimized: defer.
            assert!(fresh.take_shell_recovery(&state, true));
            fresh.state = PlayerState::default(); // New document, same process.
            state.title = "Another track".into();
            assert!(!fresh.take_shell_recovery(&state, true));
        }
    }

    #[test]
    fn recovery_keeps_the_playing_track_and_existing_queue_but_leaves_sign_in_alone() {
        let track = "https://music.youtube.com/watch?v=current";
        let queue = "https://music.youtube.com/watch?v=current&list=queue&index=2";
        let current = queue.parse().unwrap();
        assert_eq!(shell_recovery_url(&current, track).unwrap().as_str(), queue);
        for url in [
            "https://music.youtube.com/",
            "https://music.youtube.com/watch?v=previous",
        ] {
            let current = url.parse().unwrap();
            assert_eq!(shell_recovery_url(&current, track).unwrap().as_str(), track);
            assert_eq!(shell_recovery_url(&current, "").unwrap(), current);
        }
        assert!(
            shell_recovery_url(&"https://accounts.google.com/".parse().unwrap(), track).is_none()
        );
        assert!(shell_recovery_url(&current, "https://example.com/").is_none());
    }

    #[test]
    fn page_errors_are_private_and_limited_across_reloads() {
        assert_eq!(
            page_error_label("error:TypeError"),
            Some(("error", "TypeError"))
        );
        assert_eq!(
            page_error_label("unhandledrejection:TrustedTypes"),
            Some(("unhandledrejection", "TrustedTypes"))
        );
        assert_eq!(
            page_error_label("error:https://music.youtube.com/watch?v=private"),
            Some(("error", "Other"))
        );
        assert!(page_error_label("private:TypeError").is_none());
        assert!(page_error_label(&format!("error:{}", "x".repeat(65))).is_none());
        let mut player = Player::default();
        for _ in 0..5 {
            assert!(player.page_errors.allow(100));
        }
        player.state = PlayerState::default();
        assert!(!player.page_errors.allow(159));
        assert!(player.page_errors.allow(160));
    }

    #[test]
    fn music_page_gets_no_device_or_prompt_permissions_but_keeps_playback() {
        for k in [
            K::Microphone,
            K::Camera,
            K::Geolocation,
            K::Notifications,
            K::ClipboardRead,
        ] {
            assert_eq!(music_permission(k), R::Deny, "{k}");
        }
        // Playback and DRM-protected content must keep working.
        for k in [K::Autoplay, K::MediaKeySystemAccess, K::Other] {
            assert_eq!(music_permission(k), R::Default, "{k}");
        }
    }

    /// Quoted words following `marker` in `src` (`invoke('x'`, `data-cmd="x"`).
    fn quoted_after<'a>(src: &'a str, marker: &str) -> Vec<&'a str> {
        src.split(marker)
            .skip(1)
            .filter_map(|s| s.split(['\'', '"']).next())
            .collect()
    }

    fn permissions(capability: &str) -> Vec<String> {
        let v: serde_json::Value = serde_json::from_str(capability).unwrap();
        v["permissions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect()
    }

    /// Every command a page invokes is registered, declared in the app manifest
    /// and granted by that page's capability; music.youtube.com gets nothing more
    /// than its two commands. A renamed command fails here, not silently at run time.
    #[test]
    fn pages_invoke_only_registered_commands_their_capability_grants() {
        let main = include_str!("main.rs");
        let handlers = main.split("generate_handler![").nth(1).unwrap();
        let handlers = handlers.split(']').next().unwrap();
        let manifest = include_str!("../build.rs");
        let remote = include_str!("../capabilities/remote.json");
        let pages = [
            (include_str!("../../../assets/bridge.js"), remote),
            (include_str!("../../../assets/titlebar.js"), remote),
            (
                include_str!("../ui/options.html"),
                include_str!("../capabilities/options.json"),
            ),
            (
                include_str!("../ui/mini.html"),
                include_str!("../capabilities/mini.json"),
            ),
        ];
        for (src, capability) in pages {
            let granted = permissions(capability);
            let invoked = quoted_after(src, "invoke('");
            assert!(!invoked.is_empty());
            for cmd in invoked {
                assert!(handlers.contains(cmd), "{cmd} is not in generate_handler!");
                assert!(
                    manifest.contains(&format!("\"{cmd}\"")),
                    "{cmd} is not in build.rs"
                );
                let perm = format!("allow-{}", cmd.replace('_', "-"));
                assert!(
                    granted.contains(&perm),
                    "{perm} missing from the capability"
                );
            }
        }
        let v: serde_json::Value = serde_json::from_str(remote).unwrap();
        assert_eq!(
            v["remote"]["urls"],
            serde_json::json!(["https://music.youtube.com/*"])
        );
        assert_eq!(v["webviews"], serde_json::json!(["ytm"]));
        let mut remote = permissions(remote);
        remote.sort();
        assert_eq!(
            remote,
            [
                "allow-player-event",
                "allow-window-control",
                "core:window:allow-internal-toggle-maximize",
                "core:window:allow-start-dragging",
            ]
        );
    }

    /// Actions and mini-player commands sent by the pages have a match arm.
    #[test]
    fn page_actions_are_handled() {
        let main = include_str!("main.rs");
        let titlebar = include_str!("../../../assets/titlebar.js");
        let buttons = titlebar.split("const BUTTONS = [").nth(1).unwrap();
        let buttons = buttons.split("];").next().unwrap();
        let options = include_str!("../ui/options.html");
        let mut actions = quoted_after(buttons, "['");
        actions.extend(quoted_after(options, "data-win=\""));
        actions.extend(quoted_after(options, "action: '"));
        let mini = include_str!("../ui/mini.html");
        let mut cmds = quoted_after(mini, "data-cmd=\"");
        cmds.extend(quoted_after(mini, "cmd: '"));
        assert!(
            actions.len() >= 5 && cmds.len() >= 10,
            "{actions:?} {cmds:?}"
        );
        for name in actions.into_iter().chain(cmds) {
            assert!(
                main.contains(&format!("\"{name}\" =>")),
                "{name} has no match arm"
            );
        }
    }
}
