//! Options page (`ui/options.html`) and the commands it calls. It is a child
//! webview laid over the main window (not a separate window), created on demand
//! and destroyed on close: no cost while closed, music keeps playing underneath.

use serde::Serialize;
use tauri::{AppHandle, LogicalPosition, Manager, WebviewBuilder, WebviewUrl};
use ytm_core::{AudioFx, EQ_BANDS, LANGUAGES, Settings};

use crate::{BG, shared};

pub const LABEL: &str = "options";
/// Project page shown in Options > About (its `/issues` page is the second button).
pub const REPO_URL: Option<&str> = Some("https://github.com/inerthel-agi/youtube-music-desktop");

#[derive(Serialize)]
pub struct OptionsView {
    settings: Settings,
    languages: &'static [(&'static str, &'static str)],
    restart_needed: bool,
    eq_bands: [f32; 10],
    version: &'static str,
    webview_version: String,
    repo_url: Option<&'static str>,
    /// Port of the running OBS widget server (`None`: off or the port is taken).
    obs_port: Option<u16>,
}

#[derive(Serialize)]
pub struct Updated {
    settings: Settings,
    restart_needed: bool,
    /// Shortcuts Windows refused (already used by another app).
    hotkeys_refused: Vec<String>,
    obs_port: Option<u16>,
}

#[tauri::command]
pub fn get_settings(app: AppHandle) -> Result<OptionsView, String> {
    let shared = shared(&app);
    let settings = shared
        .settings
        .lock()
        .map_err(|_| "settings lock poisoned")?
        .clone();
    Ok(OptionsView {
        restart_needed: settings.needs_restart(&shared.launch),
        settings,
        languages: LANGUAGES,
        eq_bands: EQ_BANDS,
        version: env!("CARGO_PKG_VERSION"),
        webview_version: tauri::webview_version().unwrap_or_default(),
        repo_url: REPO_URL,
        obs_port: crate::obs_port(&app),
    })
}

/// Applies a full settings object from the Options page (untrusted input:
/// sanitized first) and performs the side effects of what changed.
#[tauri::command]
pub fn update_settings(app: AppHandle, settings: Settings) -> Result<Updated, String> {
    let mut new = settings.sanitized();
    let shared = shared(&app);
    let old = {
        let mut cur = shared
            .settings
            .lock()
            .map_err(|_| "settings lock poisoned")?;
        // Owned by the app, not the page.
        new.window = cur.window;
        new.last_track = cur.last_track.clone();
        std::mem::replace(&mut *cur, new.clone())
    };
    if old.close_to_tray != new.close_to_tray {
        crate::tray::set_close_to_tray_checked(&app, new.close_to_tray);
    }
    #[cfg(windows)]
    if old.launch_at_startup != new.launch_at_startup
        && let Err(e) = crate::win::set_autostart(new.launch_at_startup)
    {
        log::error!("start with Windows not updated: {e}");
    }
    if old.audio != new.audio || old.pause_on_device_change != new.pause_on_device_change {
        crate::apply_page_settings(&app);
    }
    let mut hotkeys_refused = Vec::new();
    #[cfg(windows)]
    if old.hotkeys != new.hotkeys {
        // Sync command: already on the UI thread that owns the hotkeys.
        hotkeys_refused = crate::win::register_hotkeys(&new.hotkeys);
    }
    if old.discord != new.discord {
        crate::set_discord(&app, new.discord);
    }
    if old.obs_widget != new.obs_widget || old.obs_port != new.obs_port {
        crate::set_obs(&app, new.obs_widget.then_some(new.obs_port));
    }
    crate::save_settings(&app);
    Ok(Updated {
        restart_needed: new.needs_restart(&shared.launch),
        settings: new,
        hotkeys_refused,
        obs_port: crate::obs_port(&app),
    })
}

#[tauri::command]
pub fn audio_preset(name: String) -> Result<AudioFx, String> {
    AudioFx::preset(&name).ok_or_else(|| format!("unknown preset {name:?}"))
}

#[tauri::command]
pub fn eq_preset(name: String) -> Result<[f32; 10], String> {
    ytm_core::eq_preset(&name).ok_or_else(|| format!("unknown equalizer preset {name:?}"))
}

/// Opens the project page (and its issues) or the running OBS widget in the
/// default browser. No other URL is accepted.
#[tauri::command]
pub fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    let repo = REPO_URL.is_some_and(|r| url == r || url == format!("{r}/issues"));
    let obs = crate::obs_port(&app).is_some_and(|p| url == format!("http://127.0.0.1:{p}/"));
    if !repo && !obs {
        return Err("URL not allowed".into());
    }
    #[cfg(windows)]
    crate::win::open_url(&url);
    Ok(())
}

#[tauri::command]
pub fn restart_app(app: AppHandle) {
    crate::save_settings(&app);
    app.restart();
}

pub fn open(app: &AppHandle) {
    if let Some(v) = app.get_webview(LABEL) {
        let _ = v.set_focus();
        return;
    }
    let Some(window) = crate::main_window(app) else {
        return;
    };
    crate::show_main(app);
    let size = match (window.inner_size(), window.scale_factor()) {
        (Ok(s), Ok(f)) => s.to_logical::<f64>(f),
        _ => return log::error!("cannot read the main window size"),
    };
    let args = shared(app).browser_args.clone();
    let builder = WebviewBuilder::new(LABEL, WebviewUrl::App("options.html".into()))
        .background_color(BG)
        .additional_browser_args(&args)
        .auto_resize();
    match window.add_child(builder, LogicalPosition::new(0.0, 0.0), size) {
        Ok(v) => {
            let _ = v.set_focus();
        }
        Err(e) => log::error!("cannot open options: {e}"),
    }
}

pub fn close(app: &AppHandle) {
    if let Some(v) = app.get_webview(LABEL) {
        let _ = v.close();
    }
    if let Some(m) = crate::music(app) {
        let _ = m.set_focus();
    }
}
