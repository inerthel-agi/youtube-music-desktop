//! UI-independent core shared by both apps: the player state reported by the
//! YouTube Music page, validation of bridge messages (the page is an untrusted
//! origin), and the commands sent back to the page.

use std::fmt;

use serde::{Deserialize, Serialize};

mod audio;
pub mod discord;
pub mod hotkey;
mod settings;
pub use audio::{AudioFx, BASS_MAX_DB, EQ_BANDS, EQ_MAX_DB, SPEED_RANGE, eq_preset};
pub use settings::{EqPreset, LANGUAGES, LastTrack, MIN_HEIGHT, MIN_WIDTH, Settings, WindowBounds};

/// Upper bound for one bridge message; anything larger is rejected unparsed.
pub const MAX_MSG_LEN: usize = 4096;
/// Title/artist are truncated to this many chars.
pub const MAX_TEXT_CHARS: usize = 256;
/// Volume change per click of the volume buttons.
pub const VOLUME_STEP: u8 = 10;
/// Default page loaded in the music webview.
pub const YTM_URL: &str = "https://music.youtube.com/";

/// Start page: `override_url` (env `YTM_START_URL`, used for reproducible
/// benchmarks) only if it stays on music.youtube.com, the only origin the
/// remote IPC capability is granted to.
pub fn start_url(override_url: Option<&str>) -> &str {
    override_url
        .filter(|u| u.starts_with(YTM_URL))
        .unwrap_or(YTM_URL)
}

/// Top-level navigations the music webview may follow: YouTube and Google
/// (sign-in, consent) over https, and blank pages. Anything else is cancelled:
/// the window has no address bar, so a foreign page would look like the app.
pub fn navigation_allowed(scheme: &str, host: &str) -> bool {
    let under = |domain: &str| host == domain || host.ends_with(&format!(".{domain}"));
    match scheme {
        "https" => under("youtube.com") || under("google.com"),
        "about" => host.is_empty(),
        _ => false,
    }
}

/// Playback state as published by the page (video element + Media Session).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlayerState {
    pub playing: bool,
    /// An ad is showing: title/artist then describe the ad, not the track.
    pub ad: bool,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Cover art URL, kept only if it is an https image on a Google image host.
    pub artwork: String,
    /// Links to the track, first artist and album on music.youtube.com (validated).
    pub track_url: String,
    pub artist_url: String,
    pub album_url: String,
    /// 0..=100
    pub volume: u8,
    /// Playback position and track length in whole seconds, as of this message.
    pub position: u32,
    pub duration: u32,
    /// Rating of the current track, from the player bar's like button.
    pub like: Like,
    /// Page parts the app relies on that the page no longer has (YouTube Music
    /// changed): only names from `PAGE_PARTS`.
    pub missing: Vec<String>,
}

/// Page parts `assets/bridge.js` checks for.
pub const PAGE_PARTS: &[&str] = &["progress-bar", "time-info", "player-time", "like", "next"];

/// Like state of the current track. Unknown page values read as `Indifferent`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Like {
    Like,
    Dislike,
    #[default]
    #[serde(other)]
    Indifferent,
}

impl Default for PlayerState {
    fn default() -> Self {
        // Volume of a fresh <video> element; avoids showing 0% before the page reports.
        Self {
            playing: false,
            ad: false,
            title: String::new(),
            artist: String::new(),
            album: String::new(),
            artwork: String::new(),
            track_url: String::new(),
            artist_url: String::new(),
            album_url: String::new(),
            volume: 100,
            position: 0,
            duration: 0,
            like: Like::Indifferent,
            missing: Vec::new(),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BridgeError {
    TooLong(usize),
    Invalid(String),
}

impl fmt::Display for BridgeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLong(n) => write!(f, "bridge message too long ({n} bytes)"),
            Self::Invalid(e) => write!(f, "invalid bridge message: {e}"),
        }
    }
}

impl std::error::Error for BridgeError {}

/// Parses and sanitizes one message from `assets/bridge.js`.
pub fn parse_bridge_msg(raw: &str) -> Result<PlayerState, BridgeError> {
    if raw.len() > MAX_MSG_LEN {
        return Err(BridgeError::TooLong(raw.len()));
    }
    let mut s: PlayerState =
        serde_json::from_str(raw).map_err(|e| BridgeError::Invalid(e.to_string()))?;
    s.volume = s.volume.min(100);
    // Live streams report no length; cap absurd values at one day.
    s.duration = s.duration.min(86_400);
    s.position = s.position.min(86_400);
    s.title = sanitize(&s.title);
    s.artist = sanitize(&s.artist);
    s.album = sanitize(&s.album);
    if !is_artwork_url(&s.artwork) {
        s.artwork.clear();
    }
    s.missing.retain(|m| PAGE_PARTS.contains(&m.as_str()));
    s.missing.dedup();
    for url in [&mut s.track_url, &mut s.artist_url, &mut s.album_url] {
        if !is_ytm_url(url) {
            url.clear();
        }
    }
    Ok(s)
}

/// Hosts YouTube Music serves cover art from. Anything else is dropped, so the URL
/// can be handed to Discord without relaying arbitrary links from the page.
const ARTWORK_HOSTS: &[&str] = &[".googleusercontent.com", ".ytimg.com", ".ggpht.com"];

/// A music.youtube.com page link: fixed origin, short, URL-safe characters only.
fn is_ytm_url(url: &str) -> bool {
    url.len() <= 256
        && url.strip_prefix(YTM_URL).is_some_and(|path| {
            path.chars()
                .all(|c| c.is_ascii_alphanumeric() || "/?=&_-.%".contains(c))
        })
}

fn is_artwork_url(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    url.len() <= 512
        && host
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
        && ARTWORK_HOSTS.iter().any(|h| host.ends_with(h))
}

/// Bidi embeddings, overrides and isolates: a title holding one can reverse the
/// text after it in the tray tooltip, notifications and Discord. Marks (LRM/RLM)
/// and zero-width joiners stay: real titles and emoji sequences need them.
fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

fn sanitize(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() && !is_bidi_control(*c))
        .take(MAX_TEXT_CHARS)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Automatic recovery attempts (page reload, app restart after a WebView2
/// crash): at most `max` within `window_secs`, so a page or runtime that fails
/// again at once cannot loop for ever. Times are Unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Retries {
    max: usize,
    window_secs: u64,
    times: Vec<u64>,
}

impl Retries {
    pub fn new(max: usize, window_secs: u64) -> Self {
        Self {
            max,
            window_secs,
            times: Vec::new(),
        }
    }

    /// Records an attempt at `now` and returns true, or returns false (nothing
    /// recorded) when `max` attempts already happened within the window.
    pub fn allow(&mut self, now: u64) -> bool {
        self.times
            .retain(|&t| t <= now && now - t < self.window_secs);
        if self.times.len() >= self.max {
            return false;
        }
        self.times.push(now);
        true
    }

    /// Attempt times as "t1,t2", carried to the restarted process in an
    /// environment variable.
    pub fn encode(&self) -> String {
        self.times
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Inverse of `encode`; invalid entries are dropped (the variable comes from
    /// the environment).
    pub fn decode(s: &str, max: usize, window_secs: u64) -> Self {
        let mut r = Self::new(max, window_secs);
        r.times = s
            .split(',')
            .filter_map(|t| t.trim().parse().ok())
            .take(max)
            .collect();
        r
    }
}

/// Player commands sent to the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    PlayPause,
    Play,
    Pause,
    Next,
    Previous,
    SetVolume(u8),
    Like,
    Dislike,
    ToggleMute,
    Shuffle,
    Repeat,
    /// Seek to a position in seconds.
    Seek(u32),
}

impl Command {
    /// JS evaluated in the page. Every DOM/player selector lives here.
    // ponytail: relies on YTM's player-bar markup and `#movie_player` API; when YTM
    // changes them, only this function needs updating.
    pub fn to_js(self) -> String {
        match self {
            Self::PlayPause => {
                "(v=>v&&(v.paused?v.play():v.pause()))(document.querySelector('video'))".into()
            }
            Self::Play => "document.querySelector('video')?.play()".into(),
            Self::Pause => "document.querySelector('video')?.pause()".into(),
            Self::Next => {
                "document.querySelector('ytmusic-player-bar .next-button')?.click()".into()
            }
            Self::Previous => {
                "document.querySelector('ytmusic-player-bar .previous-button')?.click()".into()
            }
            Self::SetVolume(v) => {
                let v = v.min(100);
                format!(
                    "(p=>p&&p.setVolume?p.setVolume({v}):(document.querySelector('video')||{{}}).volume={})(document.querySelector('#movie_player'))",
                    f32::from(v) / 100.0
                )
            }
            // Clicking YTM's own buttons keeps its rating, queue and repeat state in sync.
            Self::Like => {
                "document.querySelector('ytmusic-player-bar #button-shape-like button')?.click()".into()
            }
            Self::Dislike => {
                "document.querySelector('ytmusic-player-bar #button-shape-dislike button')?.click()"
                    .into()
            }
            Self::Shuffle => "document.querySelector('ytmusic-player-bar .shuffle')?.click()".into(),
            Self::Repeat => "document.querySelector('ytmusic-player-bar .repeat')?.click()".into(),
            Self::ToggleMute => {
                "(p=>p&&p.isMuted?(p.isMuted()?p.unMute():p.mute()):(v=>v&&(v.muted=!v.muted))(document.querySelector('video')))(document.querySelector('#movie_player'))".into()
            }
            Self::Seek(t) => {
                let t = t.min(86_400);
                format!(
                    "(p=>p&&p.seekTo?p.seekTo({t},true):(document.querySelector('video')||{{}}).currentTime={t})(document.querySelector('#movie_player'))"
                )
            }
        }
    }
}

impl PlayerState {
    /// "Artist — Title" for the tray and mini-player; `None` during ads or when
    /// nothing is loaded.
    pub fn track_label(&self) -> Option<String> {
        if self.ad || self.title.is_empty() {
            return None;
        }
        Some(if self.artist.is_empty() {
            self.title.clone()
        } else {
            format!("{} — {}", self.artist, self.title)
        })
    }

    pub fn volume_up(&self) -> Command {
        Command::SetVolume(self.volume.saturating_add(VOLUME_STEP).min(100))
    }

    pub fn volume_down(&self) -> Command {
        Command::SetVolume(self.volume.saturating_sub(VOLUME_STEP))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_valid_message() {
        let s = parse_bridge_msg(r#"{"playing":true,"title":"Song","artist":"Band","volume":42}"#)
            .unwrap();
        assert_eq!(
            s,
            PlayerState {
                playing: true,
                ad: false,
                title: "Song".into(),
                artist: "Band".into(),
                volume: 42,
                ..PlayerState::default()
            }
        );
    }

    #[test]
    fn missing_fields_default() {
        assert_eq!(parse_bridge_msg("{}").unwrap(), PlayerState::default());
    }

    #[test]
    fn missing_parts_keep_known_names_only() {
        let s = parse_bridge_msg(r#"{"missing":["like","<script>","like","next"]}"#).unwrap();
        assert_eq!(s.missing, ["like", "next"]);
    }

    #[test]
    fn like_state_and_unknown_values() {
        assert_eq!(
            parse_bridge_msg(r#"{"like":"like"}"#).unwrap().like,
            Like::Like
        );
        assert_eq!(
            parse_bridge_msg(r#"{"like":"dislike"}"#).unwrap().like,
            Like::Dislike
        );
        assert_eq!(
            parse_bridge_msg(r#"{"like":"<b>"}"#).unwrap().like,
            Like::Indifferent
        );
        assert!(
            Command::Seek(u32::MAX)
                .to_js()
                .contains("seekTo(86400,true)")
        );
    }

    #[test]
    fn rejects_oversized_and_malformed() {
        let big = format!(r#"{{"title":"{}"}}"#, "a".repeat(MAX_MSG_LEN));
        assert!(matches!(
            parse_bridge_msg(&big),
            Err(BridgeError::TooLong(_))
        ));
        assert!(matches!(
            parse_bridge_msg("not json"),
            Err(BridgeError::Invalid(_))
        ));
        assert!(matches!(
            parse_bridge_msg(r#"{"volume":300}"#),
            Err(BridgeError::Invalid(_))
        ));
        assert!(matches!(
            parse_bridge_msg(r#"{"volume":-1}"#),
            Err(BridgeError::Invalid(_))
        ));
    }

    #[test]
    fn sanitizes_text_and_clamps_volume() {
        let long = "é".repeat(MAX_TEXT_CHARS + 10);
        let raw = format!(r#"{{"title":" a\u0000b\n ","artist":"{long}","volume":150}}"#);
        let s = parse_bridge_msg(&raw).unwrap();
        assert_eq!(s.title, "ab");
        assert_eq!(s.artist.chars().count(), MAX_TEXT_CHARS);
        assert_eq!(s.volume, 100);
    }

    #[test]
    fn strips_bidi_overrides_but_keeps_joiners() {
        let raw = r#"{"title":"a\u202Eb\u2066c\u2069","artist":"\u05E9\u200F\u200D\u200C"}"#;
        let s = parse_bridge_msg(raw).unwrap();
        assert_eq!(s.title, "abc");
        assert_eq!(s.artist, "\u{05e9}\u{200f}\u{200d}\u{200c}");
    }

    #[test]
    fn retries_stop_a_crash_loop_and_recover_after_the_window() {
        let mut r = Retries::new(3, 600);
        assert!(r.allow(1000) && r.allow(1001) && r.allow(1002));
        assert!(
            !r.allow(1003),
            "fourth attempt within the window is refused"
        );
        assert!(r.allow(1600), "the oldest attempt left the window");
        // Survives the trip through the environment; garbage is ignored.
        let back = Retries::decode(&r.encode(), 3, 600);
        assert_eq!(back, r);
        assert!(!Retries::decode("1601,1602,x,1603", 3, 600).allow(1604));
        assert!(Retries::decode("", 3, 600).allow(5));
        // A clock set back does not count stale future times.
        assert!(Retries::decode("9000,9001,9002", 3, 600).allow(100));
    }

    #[test]
    fn artwork_only_from_google_image_hosts() {
        let ok = r#"{"artwork":"https://lh3.googleusercontent.com/abc=w544-h544"}"#;
        assert!(!parse_bridge_msg(ok).unwrap().artwork.is_empty());
        for bad in [
            "http://lh3.googleusercontent.com/a",
            "https://evil.example/a.jpg",
            "https://googleusercontent.com.evil.example/a",
            "https://user@lh3.googleusercontent.com/a",
            "javascript:alert(1)",
        ] {
            let raw = format!(r#"{{"artwork":"{bad}"}}"#);
            assert!(parse_bridge_msg(&raw).unwrap().artwork.is_empty(), "{bad}");
        }
    }

    #[test]
    fn links_only_to_music_youtube_com() {
        let raw = r#"{"track_url":"https://music.youtube.com/watch?v=abc_D-1","artist_url":"https://evil.example/x","album_url":"https://music.youtube.com/browse/MPRE\"x"}"#;
        let s = parse_bridge_msg(raw).unwrap();
        assert_eq!(s.track_url, "https://music.youtube.com/watch?v=abc_D-1");
        assert!(s.artist_url.is_empty());
        assert!(s.album_url.is_empty());
    }

    #[test]
    fn track_label_hides_ads() {
        let mut s = parse_bridge_msg(r#"{"title":"Song","artist":"Band"}"#).unwrap();
        assert_eq!(s.track_label().as_deref(), Some("Band — Song"));
        s.ad = true;
        assert_eq!(s.track_label(), None);
        assert_eq!(PlayerState::default().track_label(), None);
    }

    #[test]
    fn navigation_stays_on_youtube_and_google_sign_in() {
        for (scheme, host) in [
            ("https", "music.youtube.com"),
            ("https", "www.youtube.com"),
            ("https", "accounts.google.com"),
            ("https", "consent.youtube.com"),
            ("about", ""),
        ] {
            assert!(navigation_allowed(scheme, host), "{scheme}://{host}");
        }
        for (scheme, host) in [
            ("http", "music.youtube.com"),
            ("https", "music.youtube.com.evil.example"),
            ("https", "evilyoutube.com"),
            ("https", "evil.example"),
            ("ms-settings", ""),
            ("file", ""),
            ("javascript", ""),
            ("data", ""),
        ] {
            assert!(!navigation_allowed(scheme, host), "{scheme}://{host}");
        }
    }

    #[test]
    fn start_url_stays_on_ytm() {
        assert_eq!(start_url(None), YTM_URL);
        let watch = "https://music.youtube.com/watch?v=abc";
        assert_eq!(start_url(Some(watch)), watch);
        assert_eq!(start_url(Some("https://evil.example/")), YTM_URL);
        assert_eq!(
            start_url(Some("https://music.youtube.com.evil.example/")),
            YTM_URL
        );
    }

    #[test]
    fn volume_steps_saturate() {
        let mut s = PlayerState {
            volume: 95,
            ..Default::default()
        };
        assert_eq!(s.volume_up(), Command::SetVolume(100));
        s.volume = 5;
        assert_eq!(s.volume_down(), Command::SetVolume(0));
        assert!(Command::SetVolume(200).to_js().contains("setVolume(100)"));
        assert!(Command::SetVolume(50).to_js().contains("volume=0.5"));
    }
}
