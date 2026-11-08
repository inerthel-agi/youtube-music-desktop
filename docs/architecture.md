# Architecture

Windows 11 YouTube Music player. Priorities used to justify every decision:
(1) stability, (2) YouTube Music compatibility, (3) low resource use,
(4) maintainability, (5) user experience.

**Decision (2026-11-06): the MVP builds on the baseline prototype, now `apps/desktop` (Tauri 2 + WebView2).** The GPUI
hybrid cost +60 MB private memory and +225 MB commit, has an intermittent shutdown bug, and
rendered nothing the page does not already show (see `prototype-report.md`). `apps/hybrid` stays
as a frozen experiment, to revisit once tauri-plugin-gpui is validated on Windows. Native extras
(tray, shortcuts, mini-player, settings) go through Tauri.

## Layout

```
crates/ytm-core/   UI-independent: PlayerState, bridge message validation, Command -> JS, start URL. Unit tested.
apps/hybrid/       Tauri 2 + tauri-plugin-gpui: GPUI-rendered window hosting one child WebView2 (webview_frame).
apps/desktop/      Tauri 2 + one WebView2 window (package `youtube-music`; the prototype's "baseline").
assets/bridge.js   Script injected into music.youtube.com (shared by both apps).
tools/bench.ps1    Process-tree RAM/CPU/GPU sampler (works on YTMDesktop too).
tools/bench-suite.ps1  Idle / playing / minimized runs for both prototypes (+ YTMDesktop idle).
docs/              This file + prototype report with measurements.
```

## Components

| Layer | Owner | Role |
|---|---|---|
| Application core | Tauri 2 (TAO event loop) | Lifecycle, windows, commands, logging (`tauri-plugin-log`, local only) |
| Native UI | GPUI via `tauri-plugin-gpui` | Window renderer (wgpu, Vulkan on this machine); demand-driven redraw. Currently only lays out the webview |
| Music runtime | WebView2 (one instance) | Official music.youtube.com: session, playback, controls, library, navigation |
| Integration | `ytm-core` + `bridge.js` | Page -> Rust state events (for tray / mini-player / shortcuts later) |

No native player bar: YouTube Music already shows its own player bar; a second
one duplicated it (removed after review). Native surfaces are reserved for what
the page cannot provide: title bar, tray, mini-player, settings.

## Data flow (event-driven playback)

```
music.youtube.com  --bridge.js: <video> events + MediaSession metadata setter-->  invoke('player_event', json)
                                                                                      |
                                       ytm-core::parse_bridge_msg (size cap, schema, sanitize)
                                                                                      |
                                       stored in the app, logged on change (no redraw: nothing renders it yet)

Future native controls -> ytm_core::Command::to_js() -> run_on_main_thread -> webview.eval(js)
(tested in the prototype with a GPUI bar: play/pause, next, volume worked).
```

Decisions:

- **Tauri 2.12.5, not 3.0-alpha** — stability (1).
- **One WebView2, created once, never recreated** — memory (3) and session stability (1). Its
  user data folder is Tauri's default under `%LOCALAPPDATA%\dev.ytm-desktop.<variant>`, so the
  Google session persists.
- **Remote IPC limited to one command.** `build.rs` declares an app manifest so `player_event`
  becomes the permission `allow-player-event`; `capabilities/remote.json` grants only that to
  `https://music.youtube.com/*`. The payload is treated as untrusted (length cap, typed schema,
  control characters stripped, volume range) — security, (1).
- **State comes from the Media Session metadata** the page already publishes to Windows (the same
  data the Windows media flyout shows) and the `<video>` element events. No DOM scraping for
  read-only data; dedup in JS and in Rust avoids redundant IPC — (2), (3).
- **Empty shell recovery**: independently of playback events, `bridge.js` checks the top bar
  and player bar every 2 s while the page is visible and a track is loaded (Media Session title
  or video metadata). A healthy bar has at least 10 light-DOM descendants and visible native
  controls: logo/search/avatar buttons for the top bar, play/pause or time for the player bar.
  A bar failing these checks for at least 6 s reports `nav-bar` or `player-bar` in the validated
  `missing` list, including while paused. A healthy or hidden check clears that bar's timer.
  The native window also supplies visibility (`document.hidden` alone is insufficient for
  a child WebView2); hidden/minimized windows cannot consume the recovery attempt. Diagnostics
  include viewport dimensions and descendant counts, never track data.
  Only changes send IPC. The desktop app logs the missing parts and attempts one reload per
  process lifetime; that guard survives page reloads and track changes. It keeps the current
  URL, including queue parameters, when it identifies the playing track; otherwise it opens
  the validated track URL (or reloads the current music page if no track URL is available).
  Before navigating, the bridge snapshots the current track id, player clock and playing/paused
  state in a one-use `sessionStorage` entry, consumed at the next document's initialization
  and applied only to its matching watch URL. No settings file is changed.
  Restoration waits for the matching track and video metadata, seeks through the player API,
  then resumes only if the track was playing. A paused track blocks repeated autoplay attempts
  until explicit Play through the page or native controls. Ads are never sought through; a
  paused recovery also keeps an ad paused. Recovery supersedes the startup last-track position.
  If the snapshot cannot be saved, the page is left in place. Sign-in pages are left alone.
  Loading still interrupts audio temporarily. It is a fallback: the cause
  found on 2026-11-08 is fixed (see "Child webview size").
- **Child webview size**: the music page and the Options page are child webviews resized in
  `on_window_event` (`WindowEvent::Resized`), only when the window is not minimized. Tauri's
  `auto_resize` also applies the minimized client area (about 144x19, while `document.hidden`
  stays false); YouTube Music then switches to its mobile layout (`is-mweb-*` attributes, a
  second `ytmusic-player-bar#top-player-bar`) and after restore leaves the top bar controls
  `visibility: hidden` and the player bar `display: none`. Reproduced 3 times out of 3 with
  `SW_MINIMIZE` / `SW_RESTORE` on the player page, 0 out of 3 after the change.
- **Page errors**: `error` and `unhandledrejection` use the existing `player_event` command's
  optional `error` argument, separate from playback state. Both JS and Rust limit them to
  5 per 60 s; the Rust limit survives reloads. Rust accepts only fixed event categories and
  error names (including a Trusted Types category); arbitrary names become `Other`.
  Messages, stacks, source URLs and track metadata never enter these diagnostics. Invalid
  player messages and player debug output do not echo their payloads into the log.
- **Media keys / Windows media flyout need no code**: WebView2 registers the page's Media Session
  with Windows SMTC (verified: session `msedgewebview2.exe`, title/artist, next/prev enabled).
- **Commands use DOM/player selectors, isolated in `Command::to_js`.** These are the fragile part
  when YouTube changes its markup; only one function needs updating — (4).
- **Tauri calls are deferred out of GPUI updates** with `run_on_main_thread` (plugin rule) — (1).
- **Frameless main window** (Windows 11 rounded corners, border in the page color): the page
  itself is the title bar, so YouTube Music's background gradient reaches the top edge (a native
  caption only takes a solid color). `assets/titlebar.js` adds the caption buttons in a shadow
  root (DOM APIs only: YouTube enforces Trusted Types) and marks `ytmusic-nav-bar` as the drag
  region. It also puts Back / Forward / Home in the bar's `.left-content`, after the logo:
  `history.back()` / `history.forward()` (YTM navigates with `pushState`), enabled from the
  Navigation API (`navigation.canGoBack`), and Home clicks the logo link (in-page navigation,
  playback continues). YTM gives that part a fixed width (148 px with its side menu collapsed or below 936 px), so the app lets it grow to fit the buttons (`min-width: max-content`) and hides the logo below 936 px. The remote capability gains `start-dragging`, `internal-toggle-maximize` and the
  `window_control` command (minimize / toggle maximize / close), nothing else.
- **Options inside the main window**: the main window is a `tauri::Window` with child webviews
  (`ytm` = music.youtube.com, `options` = local page laid over it while open, destroyed on close).
  The local page keeps its own capability; music.youtube.com gets no settings access.
- **Tray + close-to-tray** (default on), **lazy mini-player** (a small local page, created on
  demand and destroyed on close), **settings** in `%APPDATA%\dev.ytm-desktop.baseline\settings.json`.
- **WebView2 robustness** (`apps/desktop/src/win.rs`): renderer crash or hang -> page reload
  (verified: renderer killed, reloaded in about 1 s); browser process exit -> app restart. Both are
  rate limited (`ytm_core::Retries`): 3 reloads per 5 min, 3 crash restarts per 10 min (history
  passed to the new process in `YTM_RESTARTS`), then a notification instead of a loop. A restarted
  process waits up to 5 s for the old one to release the single-instance mutex. While
  hidden in the tray, WebView2 memory target is set to low (measured: private WS 335 -> 309 MB).
- **`YTM_START_URL`** (accepted only under `https://music.youtube.com/`) makes benchmarks play the
  same content in both variants.
- **Logs**: `max_file_size` 2 MB, keep 2 files (plugin default is 40 KB and discards history).
- **No ad blocking, no Premium/DRM circumvention, no user-agent spoofing for Google sign-in.**
- **No Node.js at build or run time.** `cargo build` only.

## Build and run

```
cargo test -p ytm-core
cargo build --release -p youtube-music -p hybrid
target\release\hybrid.exe        # or youtube-music.exe
$env:YTM_LOG="debug"             # optional log level (default info)
```

Logs: `%LOCALAPPDATA%\dev.ytm-desktop.<variant>\logs\`.

Benchmark: `tools\bench-suite.ps1` (results in `bench\`), see `docs/prototype-report.md`.

## Memory (measured 2026-11-06, `tools/mem-breakdown.ps1`, `tools/mem-final.ps1`)

Private working set of the whole tree, signed in, one run per case (page content varies, about ±10 MB):

| Configuration | Idle | Playing |
|---|---|---|
| Default (service worker blocked by `assets/lean.js`) | 282 MB | 287 MB |
| Options > Memory saver (`--js-flags=--lite-mode --renderer-process-limit=1`) | 287 MB | 266 MB |
| Memory saver + hardware acceleration off | 230 MB | 207 MB |

- Memory saver no longer uses V8 `--lite-mode` (since 0.1.2): it implies `--jitless`, which removes
  WebAssembly and slows JS about 5x. It was first suspected in the empty top/player bar failure,
  whose cause turned out to be the minimized webview size (see "Child webview size").
  The measured gain (0 to 21 MB) was within the run-to-run noise.
- The YouTube Music page renderer is the largest part (about 160–230 MB) and is out of the app's control.
- Hardware acceleration is off by default since 2026-11-06: about -50 MB (GPU process 64 -> 15 MB). The playing CPU was
  measured with the window in the background, so it does not show the cost of software video decoding.
- Disabling extra Edge features (`SpareRendererForSitePerProcess`, `BackForwardCache`, `Translate`, ...)
  gave no measurable gain and was dropped.
- Hidden or minimized: WebView2 memory target set to low (about -26 MB measured when hidden).
- Audio effects (`assets/audiofx.js`): the reverb convolver is created and connected only while reverb
  is on, and with every effect back to neutral the source bypasses the chain. Offline render of 60 s
  of stereo audio (Edge 156, `tools/audiofx-check.html` checks the routing): EQ chain 0.53 % of one
  core, plus an idle convolver 1.37 %, bypass 0.04 %.
- WebView2 vs Servo: Servo was not adopted. YouTube Music needs Media Source Extensions,
  Google sign-in and Windows media integration, which WebView2 provides today.
