//! Discord Rich Presence wire format (local IPC named pipe, no network).
//! A frame is `op: u32 LE`, `len: u32 LE`, then `len` bytes of JSON.

use serde_json::{Value, json};

/// Discord application of this app (public ID, not a secret).
pub const CLIENT_ID: &str = "1568262073035792384";

/// Small play/pause badge shown on the cover (white Material icons).
pub const PLAY_ICON: &str =
    "https://www.gstatic.com/images/icons/material/system/2x/play_arrow_white_48dp.png";
pub const PAUSE_ICON: &str =
    "https://www.gstatic.com/images/icons/material/system/2x/pause_white_48dp.png";

pub const OP_HANDSHAKE: u32 = 0;
pub const OP_FRAME: u32 = 1;
/// Responses larger than this are treated as a broken connection.
pub const MAX_FRAME: u32 = 64 * 1024;

pub fn encode(op: u32, payload: &Value) -> Vec<u8> {
    let body = payload.to_string().into_bytes();
    let mut out = Vec::with_capacity(8 + body.len());
    out.extend_from_slice(&op.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&body);
    out
}

/// `(op, len)` from an 8-byte header; `None` if the length is implausible.
pub fn decode_header(h: [u8; 8]) -> Option<(u32, u32)> {
    let op = u32::from_le_bytes([h[0], h[1], h[2], h[3]]);
    let len = u32::from_le_bytes([h[4], h[5], h[6], h[7]]);
    (len <= MAX_FRAME).then_some((op, len))
}

pub fn handshake(client_id: &str) -> Value {
    json!({ "v": 1, "client_id": client_id })
}

/// What Discord shows while a track plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Activity {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Validated cover URL (see `is_artwork_url`), or empty.
    pub artwork: String,
    /// Unix ms when the track started and when it ends (progress bar); 0 = unknown.
    pub start_ms: u64,
    pub end_ms: u64,
    /// Validated music.youtube.com links (empty = no link).
    pub track_url: String,
    pub artist_url: String,
    pub album_url: String,
    /// Shown with the "pause" icon and no progress bar.
    pub paused: bool,
    /// Text for the play/pause icon tooltip.
    pub state_text: String,
    /// Label of the "listen" button shown to other Discord users (empty = no button).
    pub button_label: String,
}

/// Discord rejects text fields shorter than 2 or longer than 128 chars.
fn field(s: &str) -> String {
    let s: String = s.chars().take(128).collect();
    if s.chars().count() < 2 {
        format!("{s}  ")
    } else {
        s
    }
}

/// `SET_ACTIVITY` in the "Listening to" layout: cover, title, artist, album line and
/// progress bar; `None` clears it.
pub fn set_activity(pid: u32, nonce: u64, track: Option<&Activity>) -> Value {
    let activity = track.map(|t| {
        let mut a = json!({ "type": 2, "details": field(&t.title), "state": field(&t.artist) });
        for (key, url) in [("details_url", &t.track_url), ("state_url", &t.artist_url)] {
            if !url.is_empty() {
                a[key] = url.clone().into();
            }
        }
        if !t.button_label.is_empty() && !t.track_url.is_empty() {
            a["buttons"] = json!([{ "label": field(&t.button_label), "url": t.track_url }]);
        }
        if !t.paused && t.end_ms > t.start_ms && t.start_ms > 0 {
            a["timestamps"] = json!({ "start": t.start_ms, "end": t.end_ms });
        }
        let mut assets = serde_json::Map::new();
        if !t.artwork.is_empty() {
            assets.insert("large_image".into(), t.artwork.clone().into());
        }
        if !t.album.is_empty() {
            assets.insert("large_text".into(), field(&t.album).into());
        }
        if !t.album_url.is_empty() {
            assets.insert("large_url".into(), t.album_url.clone().into());
        }
        // Public Material icons on Google's static host: no upload to the Discord app needed.
        assets.insert(
            "small_image".into(),
            if t.paused { PAUSE_ICON } else { PLAY_ICON }.into(),
        );
        if !t.state_text.is_empty() {
            assets.insert("small_text".into(), field(&t.state_text).into());
        }
        if !assets.is_empty() {
            a["assets"] = Value::Object(assets);
        }
        a
    });
    json!({
        "cmd": "SET_ACTIVITY",
        "args": { "pid": pid, "activity": activity },
        "nonce": nonce.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_roundtrip() {
        let f = encode(OP_FRAME, &json!({"a": 1}));
        let (op, len) = decode_header(f[..8].try_into().unwrap()).unwrap();
        assert_eq!((op, len as usize), (OP_FRAME, f.len() - 8));
        assert_eq!(&f[8..], br#"{"a":1}"#);
        let mut big = [0u8; 8];
        big[4..].copy_from_slice(&(MAX_FRAME + 1).to_le_bytes());
        assert!(decode_header(big).is_none());
    }

    #[test]
    fn activity_payload() {
        let t = Activity {
            title: "Song".into(),
            artist: "B".into(),
            album: "LP".into(),
            artwork: "https://lh3.googleusercontent.com/x".into(),
            start_ms: 1_000,
            end_ms: 200_000,
            track_url: "https://music.youtube.com/watch?v=x".into(),
            artist_url: String::new(),
            album_url: String::new(),
            paused: false,
            state_text: "Playing".into(),
            button_label: "Listen on YouTube Music".into(),
        };
        let v = set_activity(42, 7, Some(&t));
        let a = &v["args"]["activity"];
        assert_eq!(a["details"], "Song");
        assert_eq!(a["state"], "B  ");
        assert_eq!(a["assets"]["large_text"], "LP");
        assert_eq!(
            a["assets"]["large_image"],
            "https://lh3.googleusercontent.com/x"
        );
        assert_eq!(a["timestamps"]["end"], 200_000);
        assert_eq!(v["nonce"], "7");
        assert_eq!(a["details_url"], "https://music.youtube.com/watch?v=x");
        assert!(a.get("state_url").is_none());
        assert_eq!(a["assets"]["small_image"], PLAY_ICON);
        assert_eq!(
            a["buttons"][0]["url"],
            "https://music.youtube.com/watch?v=x"
        );
        let bare = Activity {
            album: String::new(),
            artwork: String::new(),
            start_ms: 0,
            end_ms: 0,
            paused: true,
            ..t
        };
        let a = set_activity(1, 1, Some(&bare))["args"]["activity"].clone();
        assert!(a.get("timestamps").is_none());
        assert_eq!(a["assets"]["small_image"], PAUSE_ICON);
        assert!(a["assets"].get("large_image").is_none());
        assert!(set_activity(42, 8, None)["args"]["activity"].is_null());
    }
}
