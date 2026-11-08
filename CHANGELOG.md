# Changelog

All notable changes to this project. Versions follow [Semantic Versioning](https://semver.org/).

## [0.1.6] - 2026-11-08

### Fixed

- The top bar and the player bar could stay empty after the window was minimized and restored.
  Minimizing shrank the page to about 144 x 19 px, so YouTube Music switched to its mobile
  layout and did not fully switch back. The page now keeps its size while the window is
  minimized (Tauri's `auto_resize` is replaced by a resize that skips the minimized state).
- Detect an empty YouTube Music top bar or player bar even while playback is paused.
  After six seconds of persistent emptiness with a track loaded, log it and attempt
  one page reload per app session, keeping the playing track URL and existing queue
  parameters when that URL already identifies the track. The reload restores the playback
  position and the playing or paused state; a paused track stays paused.

### Changed

- Log uncaught page errors and unhandled promise rejections as fixed categories only,
  without messages, stacks, URLs or track metadata. Limit these reports to five per
  minute across page loads. Player debug logs no longer include track metadata.
- Windows CI also runs the page script tests (`node --test tools/bridge-check.mjs`).

## [0.1.5] - 2026-11-07

### Fixed

- When Windows shut down or signed out with the window open, the app did not save its settings:
  "resume the last track" reopened an older track and the window position was lost. They are
  now saved on every exit.
- A settings file that could not be read was replaced by the defaults at the next save, losing
  presets and shortcuts. It is now kept as `settings.json.corrupt`, and settings are flushed to
  disk before they replace the old file.

### Changed

- Windows CI: format, Clippy without warnings, tests, page script syntax and the Web Audio
  routing check (`tools/audiofx-check.html` in headless Chrome).

## [0.1.4] - 2026-11-07

### Fixed

- With the YouTube Music side menu collapsed, the Back, Forward and Home buttons covered the
  search box. The top bar now makes room for them.
- After repeated WebView2 crashes the app reloaded the page or restarted without end. It now
  reloads at most 3 times in 5 minutes and restarts at most 3 times in 10 minutes, then shows a
  notification.
- A restart (Options, or after a WebView2 crash) could close the app instead of reopening it.
  The new process now waits for the old one to exit.
- A Discord client that stopped answering could block Rich Presence for good. The app now gives
  up after 5 seconds and reconnects later.
- Sleep timer "end of track": it compared track titles, so it did not pause when the next track
  had the same title, and paused at once when set during an ad. It now compares the video id
  (the title when there is none) and ignores ads.
- Changing or turning off a timed sleep timer left its thread waiting until the old deadline.
  The thread now ends at once.

### Changed

- The YouTube Music page can no longer ask for the microphone, camera, location, notifications,
  clipboard reading and similar permissions, and cannot download files. Playback and Google
  sign-in are not affected.
- Text direction control characters are removed from track titles shown in the tray,
  notifications, Discord and the OBS widget.
- Sound effects use less CPU: reverb runs only while it is on, and with every effect back to
  neutral the sound skips the effect chain.
- The log also warns when the previous, dislike, shuffle or repeat button, or the top bar, is
  missing from the YouTube Music page. If the top bar never appears, the app stops watching for
  it after a minute.

## [0.1.3] - 2026-11-07

### Added

- Back, Forward and Home buttons in the top bar, after the YouTube Music logo. Back and Forward
  are dimmed when there is no page to go to. Home keeps the music playing. Below 936 px of width
  (compact mode) they take the place of the logo.

## [0.1.2] - 2026-11-06

### Fixed

- With Options > Memory saver on, the top bar (search) and the player bar could stay empty. The
  option no longer turns on V8 lite mode, which disabled WebAssembly and slowed the page about 5x.
  It still limits WebView2 to one renderer process.

## [0.1.1] - 2026-11-06

### Fixed

- Discord showed a wrong time (for example "00:49 / 00:49") or stayed on an earlier track.
  YouTube Music now plays the whole queue in one video element: its clock runs across tracks and
  its length grows while a track loads. The position now comes from the player and the length
  from the player bar. This also fixes the time in the OBS widget, the mini player, the Windows
  media flyout and "resume the last track".
- The page sent an update every second, so Discord never received the new track. Updates are sent
  only on real changes again.

### Changed

- Discord: up to 5 updates go out at once (several "next" in a row), then at most 5 per 21 s,
  under Discord's limit. A steady stream of updates delays a send by 3 s at most.
- Discord: a dropped connection is detected within 2 s (was 5 s). Discord's reason for closing it
  is written to the log.
- The log warns when the YouTube Music page no longer has a part the app relies on (progress bar,
  time text, player time, like or next button).

## [0.1.0] - 2026-11-06

First release.

### Added

- Window for music.youtube.com built with Tauri 2 and the system WebView2: frameless, caption
  buttons, tray icon, close to tray, compact mode, single instance.
- Windows media session (media keys, flyout, lock screen), taskbar thumbnail buttons, track
  notifications.
- Mini player: cover, seek, shuffle, previous, play/pause, next, repeat, like, dislike, volume.
- Options: language (English and French interface), start with Windows, hardware acceleration,
  memory saver, playback speed and pitch, sleep timer, resume the last track, pause on lock or
  when headphones are unplugged.
- 10-band equalizer with built-in and personal presets; reverb, bass boost, 8D, balance, mono,
  normalize, limiter.
- 11 configurable global shortcuts.
- Discord Rich Presence for Stable, PTB and Canary, sent only to a Discord process of the same
  Windows user.
- OBS "now playing" widget served on 127.0.0.1.
- Settings export and import.
- Per-user NSIS installer and portable zip.

[Unreleased]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.6...HEAD
[0.1.6]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.5...v0.1.6
[0.1.5]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.4...v0.1.5
[0.1.4]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.3...v0.1.4
[0.1.3]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.2...v0.1.3
[0.1.2]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/inerthel-agi/youtube-music-desktop/releases/tag/v0.1.0
