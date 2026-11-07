# Security notes

Source-only audit of `apps/desktop` (then `apps/baseline`), `crates/ytm-core` and `assets` on 2026-11-06. The full run
(ledger, records, reports) is kept outside the repository in
`%USERPROFILE%\security-audit-skill\youtube-music\run-1\`. No target code was executed during the
audit (no OS sandbox on the audit host).

## Result

No confirmed vulnerability. Three leads could not be proven without a sandbox and were fixed:

| Lead | Fix | Check |
|---|---|---|
| Discord activity sent to any process owning a `discord-ipc-N` pipe name | `discord.rs` `server_is_same_user`: the pipe server must run under the same Windows user (token SID), and must answer the handshake with `evt: READY` | `pipe_server_of_this_user_is_accepted`; real Discord still connects |
| Music webview followed any top-level navigation inside a window with no address bar | `ytm_core::navigation_allowed` in `on_navigation`: https YouTube and Google hosts and `about:blank` only; page settings and the resume script limited to `music.youtube.com` | `navigation_stays_on_youtube_and_google_sign_in` |
| OBS widget: unbounded clients, blocking socket writes on the main thread | Writer thread in `obs.rs`; `push` never touches sockets; at most 8 widgets, closed ones free their slot | `serves_page_and_pushes_state_to_local_hosts_only` |

## Invariants to keep

- The `ytm` webview gets IPC only through `capabilities/remote.json` (`player_event`, `window_control`, `https://music.youtube.com/*`). Do not grant it local commands.
- Every value from the page goes through `parse_bridge_msg` before a native sink. Build scripts and JSON with serde, never with string formatting of page data.
- Local pages (`options.html`, `mini.html`, `assets/obs.html`) render remote data with `textContent` only: they hold IPC and have no CSP.
- `open_url` opens only exact allowlisted URLs.
- The OBS listener binds `127.0.0.1` and checks the `Host` header.

## Hardening, 2026-11-07

| Item | Change | Check |
|---|---|---|
| No permission policy on the music webview | `main.rs` `music_permission`: microphone, camera, location, notifications, clipboard read, screen capture, MIDI, sensors, local fonts, window management, multiple downloads and file system access are denied; autoplay and DRM key systems keep WebView2's default | `music_page_gets_no_device_or_prompt_permissions_but_keeps_playback` |
| Downloads from the music webview | Cancelled in `on_download` (YouTube Music offers none) | not run (needs the app) |
| Discord pipe reads had no deadline | `discord.rs` `read_by`: reads only what `PeekNamedPipe` reports, gives up after 5 s or when the pipe closes | `reads_stop_at_the_deadline_when_discord_hangs` |
| WebView2 crash recovery had no retry ceiling | Page reloads: 3 per 5 min; app restarts after a browser-process exit: 3 per 10 min (count carried in `YTM_RESTARTS`), then a notification and quit | `retries_stop_a_crash_loop_and_recover_after_the_window` |
| `sanitize` kept bidi controls | Embeddings, overrides and isolates (U+202A–202E, U+2066–2069) are dropped; LRM/RLM and zero-width joiners stay (real titles, emoji) | `strips_bidi_overrides_but_keeps_joiners` |
| Known CVEs in `Cargo.lock` | OSV.dev query of the 737 crates.io packages: 9 advisories (unsoundness in `glib`, unmaintained `paste`, `proc-macro-error`, `ttf-parser`, `rustybuzz`, `zvariant`, `zbus_names`). None is in the Windows build of `youtube-music` (`cargo tree -i`): Linux-only or `apps/hybrid` | rerun before a release |

## Kept as is

- `msSmartScreenProtection` stays disabled (Tauri's default arguments, kept by `browser_args`).
  SmartScreen checks top-level navigations and downloads against Microsoft's reputation service.
  Here navigations are limited to https YouTube / Google hosts (`navigation_allowed`), new windows are
  refused (wry default without a handler) and downloads are cancelled, so it would vet nothing while
  sending each visited URL to Microsoft.

## Open hardening items

- New windows (`target=_blank`, `window.open`) from the music page are refused silently (wry default);
  links to other sites do not open in the browser.
