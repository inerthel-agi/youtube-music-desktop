# Long-run validation protocol

Status: **not run** (written 2026-11-07). Run it on an interactive Windows 11 session with the
release build, nothing else using the app's single-instance mutex, and Discord open.

## Setup

1. `cargo build --release -p youtube-music` (close any running copy first: the exe is locked).
2. Start it with info logs (default) and sample it from another PowerShell:
   `.\tools\bench.ps1 -ProcessName youtube-music -Seconds 21600 -Interval 30 -Label long-6h -Gpu`
   (6 h, one sample every 30 s: private working set, commit, CPU, GPU, process count).
3. Log file: `%LOCALAPPDATA%\dev.ytm-desktop.baseline\logs\YouTube Music.log`.

## Steps (in order, during the 6 h)

| # | Action | Pass when |
|---|---|---|
| 1 | Play a long queue or radio for 6 h, window visible 1 h, minimized 2 h, hidden to the tray 3 h | Audio never stops; no `ERROR` in the log; private WS at the last hour within 15 % of the first hour |
| 2 | 100 track changes (Next hotkey, mini player, taskbar button, tray menu, media key; include two tracks with the same title) | Discord, tray tooltip, media flyout, mini player and OBS widget show each new track within 5 s |
| 3 | Options > Sound: turn on EQ, reverb, 8D, limiter; play 30 min; set all back to neutral; play 30 min | No silence or crackle at each switch; CPU back to the no-effect level after neutral |
| 4 | Pull the network (Wi-Fi off / cable out) 2 min while playing, then restore | Playback resumes or the page recovers after a reload (F5); the app keeps running |
| 5 | Start the app with the network off, then restore it | Note whether the caption buttons and drag region are there on the error page, and whether the page loads by itself (expected: not by itself, F5 needed) |
| 6 | Kill the renderer: Task Manager > the `msedgewebview2.exe` child with `--type=renderer` | Page reloads by itself (log: "renderer failed ... reloading"); after 4 kills in 5 min a notification, no loop |
| 7 | Kill the WebView2 browser process (`msedgewebview2.exe` without `--type`) | App restarts once (log: "browser process exited"); after 4 in 10 min a notification and the app quits |
| 8 | Sleep timer: 15 min, then "end of track", then change it while pending | Pauses once at the right time; changing it never pauses at the old deadline |
| 9 | With the window open, sign out of Windows and sign in again | `settings.json` holds the last track and window bounds from before sign-out; resume opens that track |
| 10 | Turn GPU acceleration on, restart, repeat 1 h of step 1; same with memory saver | Compare averages with step 1; keep a difference only if it exceeds the spread between two runs of the same setting |

## Record

Keep the CSVs and the log next to the result, write each step as passed / failed / not run, and
the 6 h private WS and commit curves (first hour vs last hour).
