//! User settings, stored as JSON in the app config directory.

use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};

use crate::audio::{AudioFx, EQ_MAX_DB};
use crate::hotkey::Hotkeys;

/// Languages offered in Options: code and native name. The app UI itself is
/// translated in English and French; other codes fall back to English there.
pub const LANGUAGES: &[(&str, &str)] = &[
    ("system", "System"),
    ("en", "English"),
    ("fr", "Français"),
    ("es", "Español"),
    ("de", "Deutsch"),
    ("it", "Italiano"),
    ("pt", "Português"),
    ("ja", "日本語"),
];

/// Smallest window size restored from settings (logical px).
pub const MIN_WIDTH: f64 = 480.0;
pub const MIN_HEIGHT: f64 = 360.0;
/// Default port of the OBS widget server (127.0.0.1 only).
pub const OBS_PORT: u16 = 9870;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Closing the main window hides it to the tray instead of quitting.
    pub close_to_tray: bool,
    /// Keep the mini-player above other windows.
    pub mini_on_top: bool,
    /// Last main window position/size (logical px).
    pub window: Option<WindowBounds>,
    /// GPU acceleration for WebView2 (restart required). Off by default: about
    /// 50 MB less RAM (GPU process 64 -> 15 MB measured), video decoded on the CPU.
    pub hardware_acceleration: bool,
    /// Lower memory at some CPU cost: V8 lite mode, one renderer process (restart required).
    pub memory_saver: bool,
    /// `system` or a language code from [`LANGUAGES`] (restart required).
    pub language: String,
    /// Start with Windows (HKCU Run key).
    pub launch_at_startup: bool,
    /// When started with Windows, stay in the tray.
    pub start_hidden: bool,
    /// Discord Rich Presence ("Listening to ...").
    pub discord: bool,
    pub audio: AudioFx,
    /// Pause when the Windows session is locked.
    pub pause_on_lock: bool,
    /// Pause when an audio output device disappears (headphones unplugged).
    pub pause_on_device_change: bool,
    /// Reopen the last track (paused, at its position) on start.
    pub resume_last: bool,
    /// Last track for `resume_last`, written by the app.
    pub last_track: Option<LastTrack>,
    /// Windows notification when the track changes (only while the window is not focused).
    pub notify_track_change: bool,
    pub hotkeys: Hotkeys,
    /// Equalizer settings saved by the user (Options > Equalizer).
    pub eq_presets: Vec<EqPreset>,
    /// Local "now playing" page for an OBS Browser source.
    pub obs_widget: bool,
    /// Port of that page on 127.0.0.1, 1024 to 65535.
    pub obs_port: u16,
}

/// Most user equalizer presets kept, and longest name.
pub const MAX_EQ_PRESETS: usize = 24;
pub const MAX_PRESET_NAME: usize = 32;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EqPreset {
    pub name: String,
    /// Gains in dB, same bands as `AudioFx::eq`.
    pub eq: [f32; 10],
    pub preamp_db: f32,
}

/// Gain clamped to the equalizer range; non-finite values become 0 dB.
fn db(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(-EQ_MAX_DB, EQ_MAX_DB)
    } else {
        0.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LastTrack {
    /// music.youtube.com watch URL (validated like bridge links).
    pub url: String,
    pub position: u32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            close_to_tray: true,
            mini_on_top: true,
            window: None,
            hardware_acceleration: false,
            memory_saver: false,
            language: "system".into(),
            launch_at_startup: false,
            start_hidden: false,
            discord: false,
            audio: AudioFx::default(),
            pause_on_lock: false,
            pause_on_device_change: false,
            resume_last: false,
            last_track: None,
            notify_track_change: false,
            hotkeys: Hotkeys::default(),
            eq_presets: Vec::new(),
            obs_widget: false,
            obs_port: OBS_PORT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct WindowBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
    pub maximized: bool,
}

impl WindowBounds {
    /// Size clamped to the minimum; non-finite values make the bounds unusable.
    pub fn sanitized(self) -> Option<Self> {
        [self.x, self.y, self.width, self.height]
            .iter()
            .all(|v| v.is_finite())
            .then(|| Self {
                width: self.width.max(MIN_WIDTH),
                height: self.height.max(MIN_HEIGHT),
                ..self
            })
    }
}

impl Settings {
    /// Clamps every value to what the app supports. Applied on load and on every
    /// update coming from the Options page.
    pub fn sanitized(mut self) -> Self {
        self.window = self.window.and_then(WindowBounds::sanitized);
        if !LANGUAGES.iter().any(|(code, _)| *code == self.language) {
            self.language = "system".into();
        }
        self.audio = self.audio.sanitized();
        self.hotkeys = self.hotkeys.sanitized();
        let mut names = Vec::new();
        self.eq_presets.retain_mut(|p| {
            p.name = p
                .name
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_PRESET_NAME)
                .collect::<String>()
                .trim()
                .to_owned();
            p.eq = p.eq.map(db);
            p.preamp_db = db(p.preamp_db);
            let key = p.name.to_lowercase();
            let keep = !p.name.is_empty() && !names.contains(&key);
            names.push(key);
            keep
        });
        self.eq_presets.truncate(MAX_EQ_PRESETS);
        if self.obs_port < 1024 {
            self.obs_port = OBS_PORT;
        }
        if self.last_track.as_ref().is_some_and(|t| {
            !t.url.starts_with("https://music.youtube.com/watch?v=") || t.url.len() > 256
        }) {
            self.last_track = None;
        }
        self
    }

    /// WebView2 browser arguments. Every webview of the app must use the same
    /// ones (they share one browser process). Keeps Tauri's default flags.
    pub fn browser_args(&self) -> String {
        let mut args =
            // HardwareMediaKeyHandling off: WebView2's own media session belongs to
            // msedgewebview2.exe and shows as "Unknown app"; the app publishes its own.
            String::from(
                "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection,HardwareMediaKeyHandling",
            );
        if !self.hardware_acceleration {
            args.push_str(" --disable-gpu --disable-gpu-compositing");
        }
        if self.memory_saver {
            // No V8 --lite-mode: it implies --jitless, which removes WebAssembly and
            // slows JS about 5x; YouTube Music then leaves its top and player bars empty.
            args.push_str(" --renderer-process-limit=1");
        }
        if self.language != "system" {
            args.push_str(" --lang=");
            args.push_str(&self.language);
        }
        args
    }

    /// Settings whose change only applies after a restart.
    pub fn needs_restart(&self, other: &Self) -> bool {
        self.hardware_acceleration != other.hardware_acceleration
            || self.memory_saver != other.memory_saver
            || self.language != other.language
    }

    /// Missing file -> defaults. Unreadable/corrupt file -> defaults plus the error,
    /// so the caller can log it; the app must still start.
    pub fn load(path: &Path) -> (Self, Option<String>) {
        match fs::read_to_string(path) {
            // A file saved by Notepad or PowerShell can start with a UTF-8 BOM.
            Ok(s) => match serde_json::from_str::<Self>(s.trim_start_matches('\u{feff}')) {
                Ok(v) => (v.sanitized(), None),
                Err(e) => (Self::default(), Some(format!("corrupt settings: {e}"))),
            },
            Err(e) if e.kind() == io::ErrorKind::NotFound => (Self::default(), None),
            Err(e) => (Self::default(), Some(format!("cannot read settings: {e}"))),
        }
    }

    /// Writes to a temp file then renames, so a crash never leaves a half file.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).map_err(io::Error::other)?,
        )?;
        fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("ytm-core-test-{}-{name}", std::process::id()))
    }

    #[test]
    fn roundtrip_and_missing_file() {
        let path = tmp("roundtrip.json");
        let _ = fs::remove_file(&path);
        assert_eq!(Settings::load(&path), (Settings::default(), None));

        let s = Settings {
            close_to_tray: false,
            window: Some(WindowBounds {
                x: 10.0,
                y: 20.0,
                width: 900.0,
                height: 700.0,
                maximized: true,
            }),
            ..Settings::default()
        };
        s.save(&path).unwrap();
        assert_eq!(Settings::load(&path), (s, None));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn eq_presets_are_named_unique_clamped_and_capped() {
        let preset = |name: &str, g: f32| EqPreset {
            name: name.into(),
            eq: [g; 10],
            preamp_db: g,
        };
        let mut list = vec![
            preset(" Mine ", 40.0),
            preset("mine", 0.0),
            preset("  ", 0.0),
            preset("Nan", f32::NAN),
        ];
        list.extend((0..40).map(|i| preset(&format!("p{i}"), 1.0)));
        let s = Settings {
            eq_presets: list,
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(s.eq_presets.len(), MAX_EQ_PRESETS);
        assert_eq!(s.eq_presets[0].name, "Mine");
        assert_eq!(s.eq_presets[0].eq[0], EQ_MAX_DB);
        assert_eq!(s.eq_presets[1].name, "Nan");
        assert_eq!(s.eq_presets[1].preamp_db, 0.0);
    }

    #[test]
    fn sanitize_rejects_unknown_language() {
        let s = Settings {
            language: "xx".into(),
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(s.language, "system");
        let s = Settings {
            obs_port: 80,
            ..Settings::default()
        }
        .sanitized();
        assert_eq!(s.obs_port, OBS_PORT);
    }

    #[test]
    fn browser_args_follow_settings() {
        let d = Settings::default();
        assert!(d.browser_args().contains("--disable-gpu"));
        let gpu = Settings {
            hardware_acceleration: true,
            ..d.clone()
        };
        assert!(!gpu.browser_args().contains("--disable-gpu"));
        assert!(!d.browser_args().contains("--lang"));
        let s = Settings {
            hardware_acceleration: true,
            language: "fr".into(),
            ..d.clone()
        };
        assert!(!s.browser_args().contains("--disable-gpu"));
        assert!(s.browser_args().ends_with("--lang=fr"));
        let saver = Settings {
            memory_saver: true,
            ..d.clone()
        };
        assert!(saver.browser_args().contains("--renderer-process-limit=1"));
        assert!(!saver.browser_args().contains("--js-flags"));
        assert!(saver.needs_restart(&d));
        assert!(s.browser_args().starts_with("--disable-features=msWebOOUI"));
        assert!(s.needs_restart(&d));
        assert!(
            !Settings {
                start_hidden: true,
                ..d.clone()
            }
            .needs_restart(&d)
        );
    }

    #[test]
    fn utf8_bom_is_accepted() {
        let path = tmp("bom.json");
        fs::write(&path, "\u{feff}{\"discord\":true}").unwrap();
        let (s, err) = Settings::load(&path);
        assert!(err.is_none());
        assert!(s.discord);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let path = tmp("corrupt.json");
        fs::write(&path, "{not json").unwrap();
        let (s, err) = Settings::load(&path);
        assert_eq!(s, Settings::default());
        assert!(err.unwrap().starts_with("corrupt settings"));
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn partial_file_keeps_defaults_and_clamps_size() {
        let path = tmp("partial.json");
        fs::write(
            &path,
            r#"{"window":{"x":0,"y":0,"width":10,"height":10,"maximized":false}}"#,
        )
        .unwrap();
        let (s, err) = Settings::load(&path);
        assert!(err.is_none());
        assert!(s.close_to_tray);
        let w = s.window.unwrap();
        assert_eq!((w.width, w.height), (MIN_WIDTH, MIN_HEIGHT));
        fs::remove_file(&path).unwrap();
    }
}
