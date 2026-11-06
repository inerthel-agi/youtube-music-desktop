# Changelog

All notable changes to this project. Versions follow [Semantic Versioning](https://semver.org/).

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

[0.1.2]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/inerthel-agi/youtube-music-desktop/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/inerthel-agi/youtube-music-desktop/releases/tag/v0.1.0
