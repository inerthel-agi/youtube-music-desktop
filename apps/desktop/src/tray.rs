//! Notification-area icon: left click shows/hides the window, the menu holds
//! player controls, the mini-player, Options, the close-to-tray setting and Quit.

use tauri::{
    AppHandle, Manager, Wry,
    menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
};
use ytm_core::{Command, Settings};

const ID: &str = "main";
const APP_NAME: &str = "YouTube Music";
/// Windows truncates tray tooltips at 127 UTF-16 units.
const TOOLTIP_CHARS: usize = 120;

/// Kept so the Options page can sync the check mark.
struct TrayItems {
    close_to_tray: CheckMenuItem<Wry>,
}

/// Menu labels: French or English (other languages fall back to English).
fn labels(settings: &Settings) -> [&'static str; 9] {
    let french = match settings.language.as_str() {
        "system" => sys_locale_is_french(),
        lang => lang == "fr",
    };
    if french {
        [
            "Afficher / Masquer",
            "Lecture / Pause",
            "Suivant",
            "Précédent",
            "Mini-lecteur",
            "Mode compact",
            "Options…",
            "Fermer dans la zone de notification",
            "Quitter",
        ]
    } else {
        [
            "Show / Hide",
            "Play / Pause",
            "Next",
            "Previous",
            "Mini player",
            "Compact mode",
            "Options…",
            "Close to tray",
            "Quit",
        ]
    }
}

fn sys_locale_is_french() -> bool {
    #[cfg(windows)]
    return crate::win::ui_lang_is_french();
    #[cfg(not(windows))]
    false
}

pub fn create(app: &AppHandle, settings: &Settings) -> tauri::Result<()> {
    let [toggle, play, next, prev, mini, compact, opts, ctt, quit] = labels(settings);
    let item = |id: &str, text: &str| MenuItem::with_id(app, id, text, true, None::<&str>);
    let sep = || PredefinedMenuItem::separator(app);
    let close_to_tray = CheckMenuItem::with_id(
        app,
        "close_to_tray",
        ctt,
        true,
        settings.close_to_tray,
        None::<&str>,
    )?;
    let menu = Menu::with_items(
        app,
        &[
            &item("toggle", toggle)?,
            &sep()?,
            &item("play_pause", play)?,
            &item("next", next)?,
            &item("previous", prev)?,
            &sep()?,
            &item("mini", mini)?,
            &item("compact", compact)?,
            &item("options", opts)?,
            &close_to_tray,
            &sep()?,
            &item("quit", quit)?,
        ],
    )?;
    app.manage(TrayItems { close_to_tray });
    let mut builder = TrayIconBuilder::with_id(ID)
        .tooltip(APP_NAME)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, e| match e.id().as_ref() {
            "toggle" => crate::toggle_main(app),
            "play_pause" => crate::send(app, Command::PlayPause),
            "next" => crate::send(app, Command::Next),
            "previous" => crate::send(app, Command::Previous),
            "mini" => crate::toggle_mini(app),
            "compact" => {
                crate::show_main(app);
                crate::features::toggle_compact(app);
            }
            "options" => crate::options::open(app),
            // The check mark toggles itself; mirror it in the settings.
            "close_to_tray" => crate::set_close_to_tray(app, !crate::close_to_tray(app)),
            "quit" => crate::quit(app),
            _ => {}
        })
        .on_tray_icon_event(|tray, e| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = e
            {
                crate::toggle_main(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

pub fn set_close_to_tray_checked(app: &AppHandle, checked: bool) {
    if let Some(items) = app.try_state::<TrayItems>()
        && let Err(e) = items.close_to_tray.set_checked(checked)
    {
        log::warn!("tray check mark not updated: {e}");
    }
}

/// Tooltip shows the current track, or the app name during ads / when idle.
pub fn set_tooltip(app: &AppHandle, label: Option<String>) {
    let Some(tray) = app.tray_by_id(ID) else {
        return;
    };
    let text = label.map_or_else(
        || APP_NAME.to_owned(),
        |l| l.chars().take(TOOLTIP_CHARS).collect(),
    );
    if let Err(e) = tray.set_tooltip(Some(text)) {
        log::warn!("tray tooltip update failed: {e}");
    }
}
