# Prototype report (Phase 2–3)

Date: 2026-11-06. Machine: Windows 11 IoT Enterprise LTSC 24H2 (26100), Ryzen 7 9800X3D (16 threads),
RTX 5070 Ti, 64 GB. WebView2 Runtime 156.0.4314.58, Rust 1.95.0, tauri 2.12.5,
tauri-plugin-gpui 0.1.0 (gpui-pre 0.3.9), YTMDesktop 2.1.0 (installed, Electron).

## Feasibility matrix

| # | Check | Result | Evidence |
|---|---|---|---|
| 1 | Tauri 2 starts on Windows 11 | PASS | `setup` 15–30 ms after process start |
| 2 | Native GPUI window | PASS | first GPUI render 395–1009 ms. wgpu picks **Vulkan** (gpui-pre-wgpu tries Vulkan/GL, not DX12) |
| 3 | WebView2 in the same app | PASS | |
| 4 | Child WebView2 in the GPUI window | PASS | `webview_frame` places it and follows resizes |
| 5 | music.youtube.com loads | PASS | page loaded at 0.9–3.1 s. A fresh profile first shows the consent page (FR) |
| 6 | Google sign-in | **NOT TESTED** | Needs the user. Known risk: Google may block embedded webviews. No user-agent spoofing |
| 7 | Play/pause, next/prev, volume | PASS | tested with a GPUI bar (since removed). Commands go through `Command::to_js`. Next: the queue moves to the next track (preceded by an ad) |
| 7b | Autoplay from a watch URL | PASS | first playback 1.4–3.3 s after launch |
| 7c | Windows media keys / SMTC | PASS (no code) | WebView2 registers an SMTC session (`msedgewebview2.exe`) with title/artist, next/prev enabled |
| 8 | Minimized playback | PASS | state stays `playing: true` for 60 s while minimized (no pause event). Audio was not checked by ear |
| 9 | Clean shutdown | **baseline PASS / hybrid FAIL (intermittent)** | baseline: all processes exit. hybrid: 2 out of about 12 closes ignored WM_CLOSE, and once the process stayed alive with no window and no `exit` log |
| 10 | RAM/CPU measured | PASS | see below |

## Measurements (`tools/bench.ps1`, sum of the whole process tree)

Not signed in, 3 s sampling, after a 20 s warm-up. WSPriv = private working set; Commit = private bytes.

### Idle (home page, nothing playing)

| | baseline | hybrid | YTMDesktop 2.1.0* |
|---|---|---|---|
| Processes | 9 | 8 | 9 |
| Working set (MB) | **722** | 774 | 1020 |
| Private WS (MB) | **290** | 351 | 378 |
| Shared WS (MB) | 432 | 424 | 642 |
| Commit (MB) | **475** | 700 | 488 |
| CPU % (of 16 threads) | 0.1 | 0.1 | 0.1 |
| GPU % | 0 | 0 | 0 |
| Process start → window shown | 84 ms | 78–103 ms | 335 ms |

*YTMDesktop runs with its own profile and settings (state unknown) and hides in the tray when closed. The comparison is indicative.

### Same content playing (music video, `YTM_START_URL`), then minimized

Hybrid figures were measured with the GPUI bar, before it was removed. The later run without the bar is invalid because it attached to the wrong process tree.

| | baseline playing | hybrid playing | baseline minimized | hybrid minimized |
|---|---|---|---|---|
| Working set (MB) | 837 | 883 | 829 | 876 |
| Private WS (MB) | 340 | 399 | 330 | 392 |
| Commit (MB) | 531 | 783 | 521 | 731 |
| CPU % | 0.5 | 0.5 | **0.1** | **0.4** |
| GPU % | 1.0 | 0.8 | **0.0** | **0.8** |

### Analysis

- **GPUI overhead vs baseline:** about +60 MB private working set (+20%) and +225–250 MB commit (+50%).
  Likely cause: GPU initialization in the process. The process has hidden windows from D3D, OpenGL
  and Vulkan probing (`temp_d3d_window`, `NVOpenGLPbuffer`, `wgpu Device Class`), and `gpui_wgpu`
  enumerates every backend. Reducing this needs a change upstream (single backend).
- **Minimized:** baseline stops rendering the video (CPU 0.1%, GPU 0%). Hybrid keeps decoding and
  rendering (CPU 0.4%, GPU 0.8%) because the child webview is never told it is hidden. A fix to
  try: hide the webview on minimize and show it again on restore.
- **vs YTMDesktop:** baseline uses −23% private WS and −29% total WS. Hybrid uses −7% private WS and −24% total WS, but has +43% commit.
- No measurement covers 2 hours of playback, signed-in use, or gaming load (not done).

## Known issues

1. **Hybrid shutdown (stability):** intermittently ignores WM_CLOSE. Once, after the window was
   destroyed, the process stayed alive. Seen once with the GPUI surface frozen (state received,
   but no repaint until a resize). Not reproduced with the instrumented plugin (frames requested and
   drawn). Root cause unknown. It sits on the plugin side (`veto_close` / runtime state), on a
   Windows path the plugin lists as untested.
2. **Media Session during ads:** the page reports the ad's title/artist as the current track.
3. Next/previous use player-bar CSS selectors, which break if YouTube changes its markup.

## Recommendation

**Decided 2026-11-06: baseline (Tauri 2 + WebView2) for the MVP.**

Stability is priority 1. For the MVP, the **baseline (Tauri 2 + WebView2)** architecture is
better: lower memory, clean shutdown, and no rendering stack to maintain. Native extras (tray,
shortcuts, mini-player as a small Tauri window) can be built without GPUI. GPUI adds cost and a
shutdown bug, and now renders nothing visible (the native player bar duplicated YouTube Music's own
and was removed). Keep `apps/hybrid` as an experiment, and revisit it when tauri-plugin-gpui is
validated on Windows.

## Reproduce

```
cargo test -p ytm-core
cargo build --release -p baseline -p hybrid
.\tools\bench-suite.ps1            # CSV + summaries in bench\
.\tools\close-loop.ps1 -App hybrid
```
