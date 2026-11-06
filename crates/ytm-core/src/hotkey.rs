//! Global hotkeys as text ("Ctrl+Alt+P") <-> Win32 `RegisterHotKey` values.

use serde::{Deserialize, Serialize};

pub const MOD_ALT: u32 = 0x1;
pub const MOD_CONTROL: u32 = 0x2;
pub const MOD_SHIFT: u32 = 0x4;
pub const MOD_WIN: u32 = 0x8;

/// One configurable shortcut per action; empty = not registered.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Hotkeys {
    pub play_pause: String,
    pub next: String,
    pub previous: String,
    pub volume_up: String,
    pub volume_down: String,
    pub like: String,
    pub dislike: String,
    pub mute: String,
    pub shuffle: String,
    pub repeat: String,
    /// Shows or hides the main window.
    pub show_hide: String,
}

/// Number of configurable shortcuts; hotkey ids are 1..=COUNT.
pub const COUNT: usize = 11;

impl Hotkeys {
    /// `(action, shortcut)` pairs, in a fixed order (index = hotkey id - 1).
    /// New actions go at the end so existing ids keep their meaning.
    pub fn entries(&self) -> [(&'static str, &str); COUNT] {
        [
            ("play_pause", &self.play_pause),
            ("next", &self.next),
            ("previous", &self.previous),
            ("volume_up", &self.volume_up),
            ("volume_down", &self.volume_down),
            ("like", &self.like),
            ("dislike", &self.dislike),
            ("mute", &self.mute),
            ("shuffle", &self.shuffle),
            ("repeat", &self.repeat),
            ("show_hide", &self.show_hide),
        ]
    }

    /// Drops shortcuts that do not parse, so only valid ones are stored.
    pub fn sanitized(mut self) -> Self {
        for s in [
            &mut self.play_pause,
            &mut self.next,
            &mut self.previous,
            &mut self.volume_up,
            &mut self.volume_down,
            &mut self.like,
            &mut self.dislike,
            &mut self.mute,
            &mut self.shuffle,
            &mut self.repeat,
            &mut self.show_hide,
        ] {
            match parse(s) {
                Some(hk) => *s = hk.to_string(),
                None => s.clear(),
            }
        }
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Hotkey {
    pub modifiers: u32,
    pub vk: u32,
}

const NAMED: &[(&str, u32)] = &[
    ("Space", 0x20),
    ("PageUp", 0x21),
    ("PageDown", 0x22),
    ("End", 0x23),
    ("Home", 0x24),
    ("Left", 0x25),
    ("Up", 0x26),
    ("Right", 0x27),
    ("Down", 0x28),
    ("Insert", 0x2D),
    ("Delete", 0x2E),
];

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_default(),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        _ => NAMED
            .iter()
            .find(|(_, v)| *v == vk)
            .map(|(n, _)| (*n).to_owned())
            .unwrap_or_default(),
    }
}

fn key_vk(name: &str) -> Option<u32> {
    let upper = name.to_ascii_uppercase();
    if upper.len() == 1 {
        let c = upper.as_bytes()[0];
        if c.is_ascii_alphanumeric() {
            return Some(u32::from(c));
        }
    }
    if let Some(n) = upper.strip_prefix('F').and_then(|n| n.parse::<u32>().ok())
        && (1..=24).contains(&n)
    {
        return Some(0x6F + n);
    }
    NAMED
        .iter()
        .find(|(n, _)| n.eq_ignore_ascii_case(name))
        .map(|(_, v)| *v)
}

/// Parses "Ctrl+Alt+Right". At least one modifier is required so a global
/// shortcut never steals a plain key from other apps.
pub fn parse(text: &str) -> Option<Hotkey> {
    let mut modifiers = 0;
    let mut vk = None;
    for part in text.split('+').map(str::trim) {
        match part.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => modifiers |= MOD_CONTROL,
            "alt" => modifiers |= MOD_ALT,
            "shift" => modifiers |= MOD_SHIFT,
            "win" | "meta" | "super" => modifiers |= MOD_WIN,
            _ if vk.is_none() => vk = Some(key_vk(part)?),
            _ => return None, // two keys
        }
    }
    (modifiers != 0).then_some(Hotkey { modifiers, vk: vk? })
}

impl std::fmt::Display for Hotkey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (bit, name) in [
            (MOD_CONTROL, "Ctrl"),
            (MOD_ALT, "Alt"),
            (MOD_SHIFT, "Shift"),
            (MOD_WIN, "Win"),
        ] {
            if self.modifiers & bit != 0 {
                write!(f, "{name}+")?;
            }
        }
        f.write_str(&key_name(self.vk))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_normalizes() {
        let hk = parse("alt + ctrl+right").unwrap();
        assert_eq!(
            hk,
            Hotkey {
                modifiers: MOD_ALT | MOD_CONTROL,
                vk: 0x27
            }
        );
        assert_eq!(hk.to_string(), "Ctrl+Alt+Right");
        assert_eq!(parse("Ctrl+Shift+p").unwrap().to_string(), "Ctrl+Shift+P");
        assert_eq!(parse("Win+F12").unwrap().vk, 0x7B);
        assert_eq!(parse("Ctrl+7").unwrap().vk, 0x37);
    }

    #[test]
    fn rejects_unsafe_or_invalid() {
        for bad in [
            "P",
            "Right",
            "Ctrl+Alt",
            "Ctrl+A+B",
            "Ctrl+F25",
            "Ctrl+Nope",
            "",
        ] {
            assert!(parse(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn sanitize_keeps_only_valid() {
        let h = Hotkeys {
            play_pause: "ctrl+alt+space".into(),
            next: "Right".into(),
            ..Hotkeys::default()
        }
        .sanitized();
        assert_eq!(h.play_pause, "Ctrl+Alt+Space");
        assert!(h.next.is_empty());
    }
}
