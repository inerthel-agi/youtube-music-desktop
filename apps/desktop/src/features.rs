//! Playback helpers driven by player events and system events: sleep timer,
//! track-change notification, last-track resume, global hotkeys, session lock,
//! compact mode and the page-side settings.

use std::{
    sync::{
        Mutex,
        mpsc::{self, RecvTimeoutError},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::Serialize;
use tauri::{AppHandle, Manager};
use ytm_core::{Command, LastTrack, PlayerState, Settings, WindowBounds};

use crate::{main_window, send, shared};

/// Sleep timer state. A newer `generation` keeps an older deadline from pausing.
#[derive(Default)]
pub struct Sleep {
    generation: u64,
    mode: String,
    /// "End of track" is armed: pause once another track plays than this one
    /// (`None` until a track is known: armed during an ad or with nothing loaded).
    end_of_track: Option<Option<TrackKey>>,
    until_ms: Option<u64>,
    /// Held while a timed sleep is pending; dropping it wakes and ends its thread.
    cancel: Option<mpsc::Sender<()>>,
}

/// Track identity for "end of track": watch URL (video id) and title.
type TrackKey = (String, String);

/// `None` during ads (the page then reports the ad's title, while the video id
/// is already the next track's) and when nothing is loaded.
fn track_key(s: &PlayerState) -> Option<TrackKey> {
    (!s.ad && !s.title.is_empty()).then(|| (s.track_url.clone(), s.title.clone()))
}

/// Another track: by video id when both have one (two tracks can share a
/// title), else by title.
fn is_other_track(a: &TrackKey, b: &TrackKey) -> bool {
    if a.0.is_empty() || b.0.is_empty() {
        a.1 != b.1
    } else {
        a.0 != b.0
    }
}

/// Feeds a player state to an armed "end of track"; true when the pause is due.
/// The first known track becomes the reference.
fn end_of_track_due(reference: &mut Option<TrackKey>, s: &PlayerState) -> bool {
    let Some(key) = track_key(s) else {
        return false;
    };
    match reference {
        Some(r) => is_other_track(r, &key),
        None => {
            *reference = Some(key);
            false
        }
    }
}

/// Waits `d`; false when cancelled first (the sender was dropped).
fn sleep_unless_cancelled(cancel: &mpsc::Receiver<()>, d: Duration) -> bool {
    matches!(cancel.recv_timeout(d), Err(RecvTimeoutError::Timeout))
}

#[derive(Serialize)]
pub struct SleepView {
    mode: String,
    until_ms: Option<u64>,
}

/// Window bounds before entering compact mode (`None` = not compact).
#[derive(Default)]
pub struct Compact(Option<WindowBounds>);

const SLEEP_MINUTES: [u64; 6] = [15, 30, 45, 60, 90, 120];

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn view(s: &Sleep) -> SleepView {
    SleepView {
        mode: if s.mode.is_empty() {
            "off".into()
        } else {
            s.mode.clone()
        },
        until_ms: s.until_ms,
    }
}

#[tauri::command]
pub fn get_sleep_timer(app: AppHandle) -> Result<SleepView, String> {
    let sleep = app.state::<Mutex<Sleep>>();
    let s = sleep.lock().map_err(|_| "sleep lock poisoned")?;
    Ok(view(&s))
}

/// `mode`: "off", "end" (end of the current track) or a number of minutes.
#[tauri::command]
pub fn set_sleep_timer(app: AppHandle, mode: String) -> Result<SleepView, String> {
    let minutes = match mode.as_str() {
        "off" | "end" => None,
        m => Some(
            m.parse::<u64>()
                .ok()
                .filter(|m| SLEEP_MINUTES.contains(m))
                .ok_or("invalid sleep timer")?,
        ),
    };
    let current = shared(&app)
        .player
        .lock()
        .ok()
        .and_then(|p| track_key(&p.state));
    let sleep = app.state::<Mutex<Sleep>>();
    let mut s = sleep.lock().map_err(|_| "sleep lock poisoned")?;
    s.generation += 1;
    s.mode = mode.clone();
    s.end_of_track = (mode == "end").then_some(current);
    s.until_ms = minutes.map(|m| now_ms() + m * 60_000);
    // Ends the previous timer's thread at once (its sender is dropped).
    s.cancel = None;
    if let Some(m) = minutes {
        let generation = s.generation;
        let app = app.clone();
        let (tx, rx) = mpsc::channel();
        s.cancel = Some(tx);
        // At most one sleeping thread (no CPU): a newer setting ends the older one.
        let _ = thread::Builder::new()
            .name("sleep-timer".into())
            .spawn(move || {
                if !sleep_unless_cancelled(&rx, Duration::from_secs(m * 60)) {
                    return;
                }
                let sleep = app.state::<Mutex<Sleep>>();
                let Ok(mut s) = sleep.lock() else { return };
                if s.generation == generation {
                    *s = Sleep {
                        generation,
                        ..Sleep::default()
                    };
                    log::info!("sleep timer: pausing");
                    send(&app, Command::Pause);
                }
            });
    }
    log::info!("sleep timer: {mode}");
    Ok(view(&s))
}

/// Called for every new player state; `prev` is the previous one.
pub fn on_state(app: &AppHandle, prev: &PlayerState, new: &PlayerState) {
    end_of_track_pause(app, new);
    let track_changed = !new.ad && !new.title.is_empty() && new.title != prev.title;
    if track_changed {
        notify_track(app, new);
    }
    remember_last_track(app, new);
}

fn end_of_track_pause(app: &AppHandle, new: &PlayerState) {
    let sleep = app.state::<Mutex<Sleep>>();
    let Ok(mut s) = sleep.lock() else { return };
    let due = s
        .end_of_track
        .as_mut()
        .is_some_and(|reference| end_of_track_due(reference, new));
    if due {
        let generation = s.generation + 1;
        *s = Sleep {
            generation,
            ..Sleep::default()
        };
        log::info!("sleep timer: end of track, pausing");
        send(app, Command::Pause);
    }
}

fn notify_track(app: &AppHandle, s: &PlayerState) {
    let enabled = shared(app)
        .settings
        .lock()
        .map(|st| st.notify_track_change)
        .unwrap_or(false);
    // Only when the window is not in front: no notification over the app itself.
    let focused = main_window(app)
        .and_then(|w| Some(w.is_visible().ok()? && w.is_focused().ok()?))
        .unwrap_or(false);
    if !enabled || focused {
        return;
    }
    #[cfg(windows)]
    {
        let body = if s.album.is_empty() {
            s.artist.clone()
        } else {
            format!("{} \u{2022} {}", s.artist, s.album)
        };
        if let Err(e) = crate::win::notify(&s.title, &body) {
            log::warn!("track notification failed: {e}");
        }
    }
}

/// Keeps the last track (URL + position) for "resume on start". Saved to disk
/// with the other settings on hide / quit.
fn remember_last_track(app: &AppHandle, s: &PlayerState) {
    if s.ad || s.track_url.is_empty() {
        return;
    }
    if let Ok(mut st) = shared(app).settings.lock() {
        st.last_track = Some(LastTrack {
            url: s.track_url.clone(),
            position: s.position,
        });
    }
}

/// Init script for "resume on start": pause on the first play and seek back.
pub fn resume_script(settings: &Settings) -> Option<String> {
    let t = settings
        .last_track
        .as_ref()
        .filter(|_| settings.resume_last)?;
    // Init scripts run in every document (sign-in pages too): keep it to the player.
    Some(format!(
        "if (window.top === window && location.hostname === 'music.youtube.com') window.__ytmResume = {{ t: {} }};",
        t.position
    ))
}

/// Page-side settings applied on every page load and on change.
pub fn page_settings_js(settings: &Settings) -> String {
    format!(
        "window.__ytmPauseOnDeviceChange = {}; {}",
        settings.pause_on_device_change,
        settings.audio.to_js()
    )
}

/// Global hotkey ids follow `Hotkeys::entries`: 1 play/pause, 2 next,
/// 3 previous, 4 volume up, 5 volume down, 6 like, 7 dislike, 8 mute,
/// 9 shuffle, 10 repeat, 11 show/hide the window.
pub fn on_hotkey(app: &AppHandle, id: usize) {
    let volume = shared(app)
        .player
        .lock()
        .map(|p| p.state.clone())
        .unwrap_or_default();
    let cmd = match id {
        1 => Command::PlayPause,
        2 => Command::Next,
        3 => Command::Previous,
        4 => volume.volume_up(),
        5 => volume.volume_down(),
        6 => Command::Like,
        7 => Command::Dislike,
        8 => Command::ToggleMute,
        9 => Command::Shuffle,
        10 => Command::Repeat,
        11 => return crate::toggle_main(app),
        _ => return,
    };
    send(app, cmd);
}

pub fn on_session_lock(app: &AppHandle) {
    let enabled = shared(app)
        .settings
        .lock()
        .map(|s| s.pause_on_lock)
        .unwrap_or(false);
    if enabled {
        log::info!("session locked: pausing");
        send(app, Command::Pause);
    }
}

/// Compact mode: a narrow window (YouTube Music switches to its small layout);
/// toggling again restores the previous size and position.
pub fn toggle_compact(app: &AppHandle) {
    let Some(w) = main_window(app) else { return };
    let compact = app.state::<Mutex<Compact>>();
    let Ok(mut c) = compact.lock() else { return };
    let Ok(scale) = w.scale_factor() else { return };
    if let Some(b) = c.0.take() {
        let _ = w.set_size(tauri::LogicalSize::new(b.width, b.height));
        let _ = w.set_position(tauri::LogicalPosition::new(b.x, b.y));
        if b.maximized {
            let _ = w.maximize();
        }
        return;
    }
    let (Ok(pos), Ok(size)) = (w.outer_position(), w.inner_size()) else {
        return;
    };
    let (pos, size) = (pos.to_logical::<f64>(scale), size.to_logical::<f64>(scale));
    c.0 = Some(WindowBounds {
        x: pos.x,
        y: pos.y,
        width: size.width,
        height: size.height,
        maximized: w.is_maximized().unwrap_or(false),
    });
    let _ = w.unmaximize();
    let _ = w.set_size(tauri::LogicalSize::new(480.0, 760.0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state(url: &str, title: &str, ad: bool) -> PlayerState {
        PlayerState {
            track_url: url.into(),
            title: title.into(),
            ad,
            ..PlayerState::default()
        }
    }

    const A: &str = "https://music.youtube.com/watch?v=aaaaaaaaaaa";
    const B: &str = "https://music.youtube.com/watch?v=bbbbbbbbbbb";

    #[test]
    fn next_track_with_the_same_title_pauses() {
        let mut r = track_key(&state(A, "Intro", false));
        assert!(!end_of_track_due(&mut r, &state(A, "Intro", false)));
        assert!(end_of_track_due(&mut r, &state(B, "Intro", false)));
    }

    #[test]
    fn ads_never_trigger_and_never_become_the_reference() {
        // Armed during an ad: the page shows the ad's title with the track's id.
        let mut r = None;
        assert!(!end_of_track_due(&mut r, &state(A, "Some ad", true)));
        assert_eq!(r, None);
        assert!(
            !end_of_track_due(&mut r, &state(A, "Song", false)),
            "the first track is the reference"
        );
        assert!(!end_of_track_due(&mut r, &state(B, "Next ad", true)));
        assert!(end_of_track_due(&mut r, &state(B, "Next song", false)));
    }

    #[test]
    fn nothing_loaded_waits_for_a_track() {
        let mut r = track_key(&PlayerState::default());
        assert_eq!(r, None);
        assert!(!end_of_track_due(&mut r, &state(A, "Song", false)));
        assert!(end_of_track_due(&mut r, &state(B, "Other", false)));
    }

    #[test]
    fn without_a_video_id_the_title_decides() {
        let mut r = track_key(&state("", "Song", false));
        // The id shows up later for the same track: not another track.
        assert!(!end_of_track_due(&mut r, &state(A, "Song", false)));
        assert!(end_of_track_due(&mut r, &state("", "Other", false)));
        // The id changes before the title does (seen on the page): due.
        let mut r = track_key(&state(A, "Song", false));
        assert!(end_of_track_due(&mut r, &state(B, "Song", false)));
    }

    #[test]
    fn replaced_timers_end_their_threads_at_once() {
        let mut pending = Vec::new();
        let mut cancel = None;
        for _ in 0..50 {
            let (tx, rx) = mpsc::channel::<()>();
            cancel = Some(tx); // drops the previous sender, like set_sleep_timer
            pending.push(thread::spawn(move || {
                sleep_unless_cancelled(&rx, Duration::from_secs(3600))
            }));
        }
        drop(cancel); // "off"
        let t = std::time::Instant::now();
        for h in pending {
            assert!(!h.join().unwrap(), "a cancelled timer must not fire");
        }
        assert!(t.elapsed() < Duration::from_secs(2));
        let (_tx, rx) = mpsc::channel::<()>();
        assert!(
            sleep_unless_cancelled(&rx, Duration::from_millis(20)),
            "the deadline fires when not cancelled"
        );
    }
}
