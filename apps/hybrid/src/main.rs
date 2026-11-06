//! Hybrid variant: Tauri 2 owns the lifecycle and the window, GPUI is the
//! window's renderer, and one WebView2 child (music.youtube.com) fills the GPUI
//! layout through `webview_frame`. The webview is created once and never
//! recreated. No GPUI player bar: YouTube Music already shows its own, and a
//! second one would duplicate it. GPUI is kept for future native-only surfaces
//! (title bar, mini-player) and to measure its cost against `baseline`.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::OnceLock;
use std::time::Instant;

use tauri::{
    AppHandle, LogicalPosition, LogicalSize, RunEvent, Webview, WebviewBuilder, WebviewUrl, Wry,
    webview::PageLoadEvent,
};
use tauri_plugin_gpui::{
    GpuiWindowExt,
    gpui::{self, Global, WeakEntity, prelude::*, rgb},
    webview_frame,
};
use ytm_core::{PlayerState, parse_bridge_msg};

const BRIDGE_JS: &str = include_str!("../../../assets/bridge.js");
/// Matches YouTube Music's background, so no flash before the page paints.
const BG: u32 = 0x030303;

static START: OnceLock<Instant> = OnceLock::new();

fn ms() -> u128 {
    START.get().map_or(0, |s| s.elapsed().as_millis())
}

struct Shell {
    webview: Option<Webview<Wry>>,
    /// Last state reported by the page; kept for the future mini-player.
    state: PlayerState,
    first_frame_logged: bool,
    first_play_logged: bool,
}

/// Lets Tauri commands reach the GPUI root view.
struct ShellRef(WeakEntity<Shell>);

impl Global for ShellRef {}

/// Runs `f` on the GPUI root, on the main thread, outside any GPUI update.
fn update_shell(
    app: &AppHandle,
    f: impl FnOnce(&mut Shell, &mut gpui::Context<Shell>) + Send + 'static,
) {
    let res = app.run_on_main_thread(move || {
        if let Err(e) = tauri_plugin_gpui::with_app(|cx| {
            let shell = cx.global::<ShellRef>().0.clone();
            let _ = shell.update(cx, f);
        }) {
            log::error!("gpui unavailable: {e}");
        }
    });
    if let Err(e) = res {
        log::error!("run_on_main_thread failed: {e}");
    }
}

impl Render for Shell {
    fn render(&mut self, _: &mut gpui::Window, _: &mut gpui::Context<Self>) -> impl IntoElement {
        if !self.first_frame_logged {
            self.first_frame_logged = true;
            log::info!("timing: first GPUI render at {} ms", ms());
        }
        gpui::div()
            .size_full()
            .bg(rgb(BG))
            .when_some(self.webview.as_ref(), |this, wv| {
                this.child(webview_frame(wv))
            })
    }
}

#[tauri::command]
fn player_event(app: AppHandle, msg: String) -> Result<(), String> {
    let state = parse_bridge_msg(&msg).map_err(|e| {
        log::warn!("{e}");
        e.to_string()
    })?;
    // Nothing rendered depends on the state: update without cx.notify().
    update_shell(&app, move |shell, _| {
        if state.playing && !shell.first_play_logged {
            shell.first_play_logged = true;
            log::info!("timing: first playback at {} ms", ms());
        }
        if shell.state != state {
            log::info!("player: {state:?}");
            shell.state = state;
        }
    });
    Ok(())
}

fn log_level() -> log::LevelFilter {
    std::env::var("YTM_LOG")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(log::LevelFilter::Info)
}

fn add_music_webview(
    app: &AppHandle,
    window: &tauri::Window<Wry>,
) -> Result<(), Box<dyn std::error::Error>> {
    let start = std::env::var("YTM_START_URL").ok();
    let url = ytm_core::start_url(start.as_deref()).parse()?;
    let builder = WebviewBuilder::new("ytm", WebviewUrl::External(url))
        .initialization_script(BRIDGE_JS)
        .on_page_load(|_, p| {
            if p.event() == PageLoadEvent::Finished {
                log::info!("timing: page loaded at {} ms ({})", ms(), p.url());
            }
        });
    let webview = window.add_child(
        builder,
        LogicalPosition::new(0., 0.),
        LogicalSize::new(1., 1.),
    )?;
    // `webview_frame` shows and positions it from the next frame on.
    webview.hide()?;
    log::info!("timing: webview created at {} ms", ms());
    update_shell(app, move |shell, cx| {
        shell.webview = Some(webview);
        cx.notify();
    });
    Ok(())
}

fn main() {
    START.get_or_init(Instant::now);
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_log::Builder::new()
                .level(log_level())
                // Default is 40 KB + KeepOne, which silently discards history.
                .max_file_size(2_000_000)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepSome(2))
                .build(),
        )
        .invoke_handler(tauri::generate_handler![player_event])
        .setup(|app| {
            log::info!("timing: setup at {} ms", ms());
            tauri_plugin_gpui::init(app)?;
            let window = tauri::WindowBuilder::new(app, "main")
                .title("YouTube Music (hybrid)")
                .inner_size(1200.0, 800.0)
                .theme(Some(tauri::Theme::Dark))
                .build()?;
            window.attach_gpui(move |cx| {
                let shell = cx.new(|_| Shell {
                    webview: None,
                    state: PlayerState::default(),
                    first_frame_logged: false,
                    first_play_logged: false,
                });
                cx.set_global(ShellRef(shell.downgrade()));
                shell
            })?;
            log::info!("timing: gpui attached at {} ms", ms());
            add_music_webview(app.handle(), &window)?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build tauri app");

    app.run(|_, event| {
        if let RunEvent::Exit = event {
            log::info!("exit after {} ms", ms());
        }
    });
}
