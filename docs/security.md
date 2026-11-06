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

## Open hardening items

- `msSmartScreenProtection` is disabled for every webview (`settings.rs` `browser_args`).
- No permission policy on the music webview: WebView2 default prompts apply.
- Discord pipe reads have no deadline; WebView2 crash recovery has no retry ceiling.
- `sanitize` keeps bidi and zero-width characters.
- Known-CVE check of `Cargo.lock` not done (needs network).
