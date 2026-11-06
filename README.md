# YouTube Music Desktop

Desktop window for the official music.youtube.com site on Windows 11. It is built with Tauri 2
and the system WebView2, without Electron or Node.js.

## Requirements

- Windows 11 (on Windows 10 the window corner and border styling is skipped).
- Microsoft Edge WebView2 Runtime (included with Windows 11).
- To build from source: Rust 1.88 or later with the MSVC toolchain
  (`stable-x86_64-pc-windows-msvc`) and Visual Studio Build Tools with the "Desktop development
  with C++" workload.

## Install

Download from the [Releases](https://github.com/inerthel-agi/youtube-music-desktop/releases) page:

- `youtube-music-<version>-x64-setup.exe`: per-user installer (no admin rights), with a Start
  menu entry and an uninstaller.
- `youtube-music-<version>-windows-x64.zip`: portable `youtube-music.exe`.

The files are not code-signed, so Windows SmartScreen shows a warning on first launch
("More info", then "Run anyway").

Build from source:

```
git clone https://github.com/inerthel-agi/youtube-music-desktop.git
cd youtube-music-desktop
cargo build --release -p youtube-music
```

The executable is `target\release\youtube-music.exe`. The installer is built with
`cargo tauri build` in `apps\desktop` (needs `tauri-cli` 2).

## Usage

Start "YouTube Music" from the Start menu, or run:

```
target\release\youtube-music.exe
```

- Sign in with the "Sign in" button of the page. The session is kept in
  `%LOCALAPPDATA%\dev.ytm-desktop.baseline\EBWebView`.
- Drag the window by the YouTube Music top bar. Double-click it to maximize.
- The minimize, maximize and close buttons are at the top right.
- Closing the window hides it in the notification area. Left-click the tray icon to show or
  hide it again.
- The tray menu has Play / Pause, Next, Previous, Mini player, Compact mode, Options, Close to tray and Quit.
- Compact mode resizes the window to 480 x 760 (YouTube Music's narrow layout); choose it again to restore.
- The window title (taskbar, Alt+Tab) shows "Artist — Title" while a track is loaded.
- The taskbar preview has Previous, Play/Pause and Next buttons.
- Media keys, the Windows media flyout and the lock screen use a media session owned by the
  app ("YouTube Music", with cover, album and progress). On first start the app creates
  `%APPDATA%\Microsoft\Windows\Start Menu\Programs\YouTube Music.lnk`; Windows reads the
  session name and icon from it. WebView2's own media session is turned off
  (`HardwareMediaKeyHandling`) because it shows as "Unknown app".
- The mini player is a small always-on-top window: cover, progress bar (click to seek),
  shuffle, previous, play/pause, next, repeat, like, dislike, volume, and a button that reopens
  the main window.
- Only one instance runs. Starting the app again shows the running window (a start with
  `--autostart` just exits).
- Options > Equalizer saves your own presets (up to 24). Options > About exports the settings to
  `Downloads\youtube-music-settings.json` (without window position and last track) and imports
  such a file.
- The gear button (top right) or the tray menu opens Options inside the main window. Back or
  Esc returns to the music; playback continues underneath.

## Configuration

Options writes `%APPDATA%\dev.ytm-desktop.baseline\settings.json`. A missing or corrupt file
falls back to the defaults. Keys marked "restart" apply after Options > Restart.

| Key | Type | Default | Effect |
|---|---|---|---|
| `language` | string | `system` | App and page language: `system`, `en`, `fr`, `es`, `de`, `it`, `pt`, `ja` (restart). The app's own labels exist in English and French. |
| `launch_at_startup` | bool | `false` | Adds `"<exe>" --autostart` to `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`. |
| `start_hidden` | bool | `false` | When started with `--autostart`, stays in the notification area. |
| `close_to_tray` | bool | `true` | Closing the window hides it instead of quitting. |
| `hardware_acceleration` | bool | `false` | Off adds `--disable-gpu --disable-gpu-compositing` to WebView2: about 50 MB less RAM, video decoded on the CPU (restart). |
| `memory_saver` | bool | `false` | Adds `--js-flags=--lite-mode --renderer-process-limit=1` to WebView2 (restart). |
| `audio.speed` | number | `1.0` | Playback rate, 0.5 to 1.5. Ads always play at 1.0. |
| `audio.preserve_pitch` | bool | `true` | Off changes the pitch with the speed ("slowed", "nightcore"). |
| `audio.reverb` | number | `0` | Reverb level, 0 to 1. |
| `audio.bass_db` | number | `0` | Low-shelf boost at 120 Hz, 0 to 12 dB. |
| `audio.normalize` | bool | `false` | Dynamics compressor on the output. |
| `audio.eq` | 10 numbers | `0` | Equalizer gains in dB (-12 to 12) at 31, 62, 125, 250, 500, 1k, 2k, 4k, 8k and 16k Hz. |
| `audio.preamp_db` | number | `0` | Gain before the equalizer, -12 to 12 dB. |
| `audio.balance` | number | `0` | Stereo balance, -1 (left) to 1 (right). |
| `audio.mono` | bool | `false` | Downmix to mono. |
| `audio.spatial` | bool | `false` | "8D" effect: the sound circles left to right every 12 s. |
| `audio.limiter` | bool | `false` | Limiter at -1 dBFS on the output. |
| `notify_track_change` | bool | `false` | Windows notification on track change, when the window is not in front. |
| `pause_on_lock` | bool | `false` | Pauses when the Windows session is locked. |
| `pause_on_device_change` | bool | `false` | Pauses when an audio output device disappears (headphones unplugged). |
| `resume_last` | bool | `false` | Reopens the last track on start, paused at its position (`last_track`, written by the app). |
| `hotkeys.*` | string | empty | Global shortcuts (`play_pause`, `next`, `previous`, `volume_up`, `volume_down`, `like`, `dislike`, `mute`, `shuffle`, `repeat`, `show_hide`), for example `Ctrl+Alt+P`. At least one modifier is required. |
| `eq_presets` | list | empty | Saved equalizer presets: `name` (1 to 32 characters, unique), `eq` (10 gains in dB), `preamp_db`. At most 24. |
| `discord` | bool | `false` | Discord Rich Presence through the local Discord pipes: cover, title, artist, album, progress, links, play/pause badge, a "Listen on YouTube Music" button. Sent to every running client (Stable, PTB, Canary); reconnects within 2 s after an account switch or a dropped connection. Sent only to a Discord process running under the same Windows account. |
| `obs_widget` | bool | `false` | Serves a "now playing" page for an OBS Browser source at `http://127.0.0.1:<obs_port>/` (cover, title, artist, album, progress; transparent background; hidden during ads). Updates are pushed as Server-Sent Events from `/events`. At most 8 widgets at once. |
| `obs_port` | number | `9870` | Port of the OBS widget, 1024 to 65535. Listens on 127.0.0.1 only. |
| `mini_on_top` | bool | `true` | Keeps the mini player above other windows. |
| `window` | object | none | Last position, size and maximized state of the main window. |

The audio effects use the Web Audio API in the page. The graph is created the first time an
effect is enabled and stays until the page reloads.

Environment variables:

| Name | Default | Effect |
|---|---|---|
| `YTM_LOG` | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`). |
| `YTM_START_URL` | `https://music.youtube.com/` | Start page. Accepted only if it starts with `https://music.youtube.com/`. |
| `YTM_BROWSER_ARGS` | computed | Replaces all WebView2 arguments. For benchmarks only. |

Logs are written to `%LOCALAPPDATA%\dev.ytm-desktop.baseline\logs\` (2 MB per file, 2 files kept).

## Development

```
cargo test -p ytm-core
.\tools\bench-suite.ps1
.\tools\close-loop.ps1 -App youtube-music
.\tools\mem-breakdown.ps1 -ProcessName youtube-music
.\tools\mem-final.ps1
```

`tools\bench-suite.ps1` writes CSV files and summaries to `bench\`. The measured results and
the architecture decision are in `docs\prototype-report.md` and `docs\architecture.md`.
The data folders keep the identifier `dev.ytm-desktop.baseline` from the prototype, so settings
and the Google session survive updates. `apps\hybrid` is a frozen GPUI experiment and is not part of the app.

## Limitations

- The Discord play/pause badge loads white Material icons from `www.gstatic.com`.
- The album comes from the page's media metadata, or from the player bar when that is empty.
  Music videos usually have no album.
- Google sign-in works in WebView2 as of 2026-11-06. Google can change this at any time.
- Next and Previous click elements of the YouTube Music page. They stop working if YouTube
  changes its markup. The selectors are in `Command::to_js` (`crates\ytm-core\src\lib.rs`).
- The drag region and the caption buttons depend on the `ytmusic-nav-bar` element of the page.
  If YouTube renames it, the window can no longer be dragged. The buttons still work.
- During ads, the page reports the ad as the current track. The tray tooltip and the mini
  player show the app name instead.
- The YouTube Music service worker is disabled (`assets\lean.js`) to save one process. Offline
  features of the website do not work.
- Like, dislike, shuffle and repeat click the buttons of the YouTube Music player bar. They stop
  working if YouTube changes its markup (selectors in `Command::to_js`).
- No choice of audio output device: the page only sees device names after a microphone
  permission, which the app does not grant.
- The sleep timer is not saved: it is cleared when the app restarts.
- The OBS widget is reachable from the same PC only: OBS on another machine cannot use it.
  Requests whose `Host` header is not `127.0.0.1:<port>` or `localhost:<port>` are refused.
- The music view follows links only to YouTube and Google pages (player, sign-in, consent). Other
  navigations are blocked and logged, because the window has no address bar.
- No installer.
- No ad blocking, and no Premium or DRM circumvention.

## License

MIT. See `LICENSE`.
