//! Windows-only APIs Tauri does not expose. Every `unsafe` block here is a
//! plain Win32/WebView2 COM call on handles owned by Tauri, made on the main
//! thread; failures are returned or logged, never ignored silently.

use tauri::{AppHandle, Manager, Runtime, Webview, Window};
use webview2_com::{
    Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        COREWEBVIEW2_PROCESS_FAILED_KIND, COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED,
        COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE, ICoreWebView2_19,
    },
    ProcessFailedEventHandler,
};
use windows::{
    Win32::{
        Foundation::{COLORREF, ERROR_FILE_NOT_FOUND, HWND},
        Globalization::GetUserDefaultUILanguage,
        Graphics::Dwm::{
            DWMWA_BORDER_COLOR, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND, DwmSetWindowAttribute,
        },
        System::Registry::{HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegSetKeyValueW},
    },
    core::{HSTRING, Interface, w},
};

/// YouTube Music's page background (#030303) as a COLORREF (0x00BBGGRR).
const YTM_BG: COLORREF = COLORREF(0x0003_0303);

/// Frameless window: rounded Windows 11 corners and a border in the page color.
/// Both attributes need Windows 11; on Windows 10 the call fails and is logged.
pub fn style_frame<R: Runtime>(window: &Window<R>) {
    let hwnd = match window.hwnd() {
        Ok(h) => HWND(h.0),
        Err(e) => return log::warn!("no hwnd for frame styling: {e}"),
    };
    let color = YTM_BG;
    let corner = DWMWCP_ROUND;
    // SAFETY: valid HWND owned by Tauri; pointers reference live locals of the stated size.
    let res = unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_WINDOW_CORNER_PREFERENCE,
            (&raw const corner).cast(),
            size_of_val(&corner) as u32,
        )
        .and_then(|()| {
            DwmSetWindowAttribute(
                hwnd,
                DWMWA_BORDER_COLOR,
                (&raw const color).cast(),
                size_of::<COLORREF>() as u32,
            )
        })
    };
    if let Err(e) = res {
        log::warn!("frame styling failed: {e}");
    }
}

/// Reloads the page when its renderer dies or hangs; restarts the app when the
/// WebView2 browser process itself exits (the webview is unusable then).
pub fn watch_process_failures<R: Runtime>(window: &Webview<R>) {
    let app = window.app_handle().clone();
    let res = window.with_webview(move |pw| {
        let handler = ProcessFailedEventHandler::create(Box::new(move |sender, args| {
            let mut kind = COREWEBVIEW2_PROCESS_FAILED_KIND::default();
            if let Some(args) = args {
                // SAFETY: COM call on the event args WebView2 passed to this handler.
                unsafe { args.ProcessFailedKind(&mut kind)? };
            }
            on_process_failed(&app, sender, kind);
            Ok(())
        }));
        let mut token = 0i64;
        // SAFETY: controller/webview are live COM objects owned by Tauri, on the main thread.
        let res = unsafe {
            pw.controller()
                .CoreWebView2()
                .and_then(|wv| wv.add_ProcessFailed(&handler, &mut token))
        };
        if let Err(e) = res {
            log::error!("cannot watch WebView2 process failures: {e}");
        }
    });
    if let Err(e) = res {
        log::error!("with_webview failed: {e}");
    }
}

fn on_process_failed<R: Runtime>(
    app: &AppHandle<R>,
    sender: Option<webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2>,
    kind: COREWEBVIEW2_PROCESS_FAILED_KIND,
) {
    if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_BROWSER_PROCESS_EXITED {
        log::error!("WebView2 browser process exited; restarting the app");
        let app = app.clone();
        // Not from inside the COM callback.
        let _ = app.clone().run_on_main_thread(move || app.restart());
    } else if kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_EXITED
        || kind == COREWEBVIEW2_PROCESS_FAILED_KIND_RENDER_PROCESS_UNRESPONSIVE
    {
        log::warn!("WebView2 renderer failed (kind {}); reloading", kind.0);
        if let Some(wv) = sender {
            // SAFETY: `sender` is the live ICoreWebView2 that raised the event.
            if let Err(e) = unsafe { wv.Reload() } {
                log::error!("reload after renderer failure failed: {e}");
            }
        }
    } else {
        // Subframe / utility / GPU process failures: WebView2 recovers on its own.
        log::warn!("WebView2 auxiliary process failed (kind {})", kind.0);
    }
}

/// Asks WebView2 to trim memory while the window is hidden (audio keeps playing).
pub fn set_low_memory<R: Runtime>(window: &Webview<R>, low: bool) {
    let level = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let res = window.with_webview(move |pw| {
        // SAFETY: live COM objects owned by Tauri, on the main thread.
        let res = unsafe {
            pw.controller()
                .CoreWebView2()
                .and_then(|wv| wv.cast::<ICoreWebView2_19>())
                .and_then(|wv| wv.SetMemoryUsageTargetLevel(level))
        };
        if let Err(e) = res {
            log::warn!("memory target level not applied: {e}");
        }
    });
    if let Err(e) = res {
        log::warn!("with_webview failed: {e}");
    }
}

const RUN_KEY: windows::core::PCWSTR = w!(r"Software\Microsoft\Windows\CurrentVersion\Run");
const RUN_VALUE: windows::core::PCWSTR = w!("YouTubeMusicDesktop");

/// Adds or removes the per-user "Run" entry (no admin rights needed).
pub fn set_autostart(enabled: bool) -> Result<(), String> {
    if !enabled {
        // SAFETY: constant, null-terminated wide strings.
        let err = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, RUN_VALUE) };
        return if err.is_ok() || err == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("registry delete failed: {err:?}"))
        };
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let cmd = format!("\"{}\" {}", exe.display(), crate::AUTOSTART_FLAG);
    let data = HSTRING::from(cmd);
    let bytes = (data.len() + 1) * size_of::<u16>(); // include the terminating NUL
    // SAFETY: `data` is a live, NUL-terminated UTF-16 buffer of `bytes` bytes.
    let err = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            RUN_KEY,
            RUN_VALUE,
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            bytes as u32,
        )
    };
    err.ok().map_err(|e| format!("registry write failed: {e}"))
}

/// The Windows display language is French (primary language id 0x0C).
pub fn ui_lang_is_french() -> bool {
    // SAFETY: no arguments, returns a LANGID.
    unsafe { GetUserDefaultUILanguage() & 0x3ff == 0x0c }
}

// ---------------------------------------------------------------------------
// Taskbar thumbnail toolbar: previous / play-pause / next under the taskbar
// preview. Buttons are (re)added on "TaskbarButtonCreated" (sent again after a
// hide/show or an Explorer restart); clicks arrive as WM_COMMAND/THBN_CLICKED
// in a window subclass. All state lives on the main (UI) thread.
// ---------------------------------------------------------------------------

use std::{cell::RefCell, sync::OnceLock};

use windows::Win32::{
    Foundation::{LPARAM, LRESULT, WPARAM},
    Graphics::Gdi::{CreateBitmap, DeleteObject},
    System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance},
    UI::{
        Shell::{
            DefSubclassProc, ITaskbarList3, SetWindowSubclass, THB_FLAGS, THB_ICON, THB_TOOLTIP,
            THBF_ENABLED, THBN_CLICKED, THUMBBUTTON, TaskbarList,
        },
        WindowsAndMessaging::{
            CreateIconIndirect, GetSystemMetrics, HICON, ICONINFO, RegisterWindowMessageW,
            SM_CXSMICON, WM_COMMAND,
        },
    },
};

const ID_PREV: u32 = 1;
const ID_PLAY: u32 = 2;
const ID_NEXT: u32 = 3;

struct Thumb {
    list: ITaskbarList3,
    hwnd: HWND,
    icons: [HICON; 4], // prev, play, pause, next
    playing: bool,
    added: bool,
}

thread_local! {
    static THUMB: RefCell<Option<Thumb>> = const { RefCell::new(None) };
}
static APP: OnceLock<AppHandle> = OnceLock::new();
static TASKBAR_CREATED: OnceLock<u32> = OnceLock::new();

#[derive(Clone, Copy)]
enum Glyph {
    Prev,
    Play,
    Pause,
    Next,
}

fn inside(g: Glyph, x: f32, y: f32) -> bool {
    // Point-in-triangle by edge signs; coordinates are 0..1.
    let tri = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        let s = |p: (f32, f32), q: (f32, f32)| (x - q.0) * (p.1 - q.1) - (p.0 - q.0) * (y - q.1);
        let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
        !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0))
    };
    let rect = |x0: f32, x1: f32| x >= x0 && x <= x1 && (0.22..=0.78).contains(&y);
    match g {
        Glyph::Play => tri((0.3, 0.2), (0.3, 0.8), (0.8, 0.5)),
        Glyph::Pause => rect(0.27, 0.43) || rect(0.57, 0.73),
        Glyph::Next => tri((0.2, 0.22), (0.2, 0.78), (0.64, 0.5)) || rect(0.66, 0.8),
        Glyph::Prev => tri((0.8, 0.22), (0.8, 0.78), (0.36, 0.5)) || rect(0.2, 0.34),
    }
}

/// White glyph on a transparent background, 4x4 supersampled.
fn make_icon(g: Glyph, size: i32) -> Option<HICON> {
    let n = size as usize;
    let mut px = vec![0u32; n * n];
    for (i, p) in px.iter_mut().enumerate() {
        let (cx, cy) = ((i % n) as f32, (i / n) as f32);
        let mut hits = 0u32;
        for sy in 0..4 {
            for sx in 0..4 {
                let x = (cx + (sx as f32 + 0.5) / 4.0) / n as f32;
                let y = (cy + (sy as f32 + 0.5) / 4.0) / n as f32;
                hits += u32::from(inside(g, x, y));
            }
        }
        let a = hits * 255 / 16;
        *p = (a << 24) | (a << 16) | (a << 8) | a; // premultiplied white, BGRA
    }
    // Monochrome mask rows are WORD-aligned.
    let mask = vec![0u8; n.div_ceil(16) * 2 * n];
    // SAFETY: buffers outlive the calls; the bitmaps are deleted once the icon copied them.
    unsafe {
        let color = CreateBitmap(size, size, 1, 32, Some(px.as_ptr().cast()));
        let mono = CreateBitmap(size, size, 1, 1, Some(mask.as_ptr().cast()));
        let info = ICONINFO {
            fIcon: true.into(),
            hbmColor: color,
            hbmMask: mono,
            ..Default::default()
        };
        let icon = CreateIconIndirect(&info).ok();
        let _ = DeleteObject(color.into());
        let _ = DeleteObject(mono.into());
        icon
    }
}

fn button(id: u32, icon: HICON, tip: &str) -> THUMBBUTTON {
    let mut b = THUMBBUTTON {
        dwMask: THB_ICON | THB_TOOLTIP | THB_FLAGS,
        iId: id,
        hIcon: icon,
        dwFlags: THBF_ENABLED,
        ..Default::default()
    };
    for (dst, src) in b.szTip.iter_mut().zip(tip.encode_utf16().take(259)) {
        *dst = src;
    }
    b
}

fn sync_buttons(t: &mut Thumb) {
    let (prev, play, pause, next) = if ui_lang_is_french() {
        ("Précédent", "Lecture", "Pause", "Suivant")
    } else {
        ("Previous", "Play", "Pause", "Next")
    };
    let middle = if t.playing {
        button(ID_PLAY, t.icons[2], pause)
    } else {
        button(ID_PLAY, t.icons[1], play)
    };
    let buttons = [
        button(ID_PREV, t.icons[0], prev),
        middle,
        button(ID_NEXT, t.icons[3], next),
    ];
    // SAFETY: valid taskbar list and HWND; the slice lives for the call.
    let res = unsafe {
        if t.added {
            t.list.ThumbBarUpdateButtons(t.hwnd, &buttons)
        } else {
            t.list.ThumbBarAddButtons(t.hwnd, &buttons)
        }
    };
    match res {
        Ok(()) => t.added = true,
        // Fails before the taskbar button exists; TaskbarButtonCreated retries.
        Err(e) => log::debug!("thumbnail buttons not set yet: {e}"),
    }
}

/// Installs the thumbnail toolbar on the main window. Main thread only.
pub fn install_thumbbar(window: &tauri::Window) -> Result<(), String> {
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    let _ = APP.set(window.app_handle().clone());
    let _ = MAIN_HWND.set(hwnd.0 as isize);
    register_session_events(hwnd);
    // SAFETY: constant NUL-terminated name.
    let msg = unsafe { RegisterWindowMessageW(w!("TaskbarButtonCreated")) };
    let _ = TASKBAR_CREATED.set(msg);
    // SAFETY: COM is initialized on the UI thread by Tauri/WebView2.
    let list: ITaskbarList3 = unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| format!("ITaskbarList3: {e}"))?;
    // SAFETY: freshly created COM object.
    unsafe { list.HrInit() }.map_err(|e| format!("HrInit: {e}"))?;
    // SAFETY: no arguments.
    let size = unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16);
    let icons = [Glyph::Prev, Glyph::Play, Glyph::Pause, Glyph::Next]
        .map(|g| make_icon(g, size).unwrap_or_default());
    THUMB.with(|t| {
        let mut thumb = Thumb {
            list,
            hwnd,
            icons,
            playing: false,
            added: false,
        };
        sync_buttons(&mut thumb);
        *t.borrow_mut() = Some(thumb);
    });
    // SAFETY: valid HWND owned by Tauri; the procedure is a plain function.
    if !unsafe { SetWindowSubclass(hwnd, Some(thumb_proc), 0x594d, 0) }.as_bool() {
        return Err("SetWindowSubclass failed".into());
    }
    Ok(())
}

/// Swaps the middle button between play and pause. Main thread only.
pub fn set_thumb_playing(playing: bool) {
    THUMB.with(|t| {
        if let Ok(mut t) = t.try_borrow_mut()
            && let Some(t) = t.as_mut()
            && t.playing != playing
        {
            t.playing = playing;
            sync_buttons(t);
        }
    });
}

unsafe extern "system" fn thumb_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    if handle_window_message(msg, wparam) {
        return LRESULT(0);
    }
    if TASKBAR_CREATED.get() == Some(&msg) {
        THUMB.with(|t| {
            if let Ok(mut t) = t.try_borrow_mut()
                && let Some(t) = t.as_mut()
            {
                t.added = false;
                sync_buttons(t);
            }
        });
    } else if msg == WM_COMMAND && ((wparam.0 >> 16) & 0xffff) as u32 == THBN_CLICKED {
        let cmd = match (wparam.0 & 0xffff) as u32 {
            ID_PREV => Some(ytm_core::Command::Previous),
            ID_PLAY => Some(ytm_core::Command::PlayPause),
            ID_NEXT => Some(ytm_core::Command::Next),
            _ => None,
        };
        if let (Some(cmd), Some(app)) = (cmd, APP.get()) {
            let app2 = app.clone();
            // Leave the window procedure before touching the webview.
            let _ = app.run_on_main_thread(move || crate::send(&app2, cmd));
            return LRESULT(0);
        }
    }
    // SAFETY: forwards the same arguments to the next procedure in the chain.
    unsafe { DefSubclassProc(hwnd, msg, wparam, lparam) }
}

// ---------------------------------------------------------------------------
// Windows media session (flyout, lock screen, media keys) owned by the app, so
// it shows as "YouTube Music" with its icon instead of WebView2's "Unknown app".
// Windows names a session from the Start menu shortcut carrying the same
// AppUserModelID, so the app sets that ID on its process and creates the
// shortcut if missing.
// ---------------------------------------------------------------------------

use windows::{
    Foundation::{TimeSpan, TypedEventHandler, Uri},
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton, SystemMediaTransportControlsButtonPressedEventArgs,
        SystemMediaTransportControlsTimelineProperties,
    },
    Storage::Streams::RandomAccessStreamReference,
    Win32::{
        Storage::EnhancedStorage::PKEY_AppUserModel_ID,
        System::{
            Com::{IPersistFile, StructuredStorage::PROPVARIANT},
            WinRT::ISystemMediaTransportControlsInterop,
        },
        UI::Shell::{
            IShellLinkW, PropertiesSystem::IPropertyStore, SetCurrentProcessExplicitAppUserModelID,
            ShellLink,
        },
    },
};

/// AppUserModelID of the app (taskbar grouping, media flyout name and icon).
pub const APP_ID: &str = "dev.ytm-desktop.baseline";

/// Must run before any window exists.
pub fn set_app_id() {
    // SAFETY: NUL-terminated string that outlives the call.
    if let Err(e) = unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(APP_ID)) } {
        log::warn!("AppUserModelID not set: {e}");
    }
}

/// Creates "YouTube Music.lnk" in the user's Start menu (with the AppUserModelID)
/// when it does not exist yet. Windows reads the media session name and icon from it.
pub fn ensure_start_menu_shortcut() -> Result<(), String> {
    let appdata = std::env::var_os("APPDATA").ok_or("APPDATA not set")?;
    let lnk = std::path::Path::new(&appdata)
        .join(r"Microsoft\Windows\Start Menu\Programs\YouTube Music.lnk");
    if lnk.exists() {
        return Ok(());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = HSTRING::from(exe.as_os_str());
    // SAFETY: COM is initialized on this thread; all strings outlive the calls.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)
            .map_err(|e| format!("ShellLink: {e}"))?;
        link.SetPath(&exe).map_err(|e| e.to_string())?;
        link.SetIconLocation(&exe, 0).map_err(|e| e.to_string())?;
        link.SetDescription(w!("YouTube Music"))
            .map_err(|e| e.to_string())?;
        let store: IPropertyStore = link.cast().map_err(|e| e.to_string())?;
        store
            .SetValue(&PKEY_AppUserModel_ID, &PROPVARIANT::from(APP_ID))
            .and_then(|()| store.Commit())
            .map_err(|e| format!("AppUserModelID on shortcut: {e}"))?;
        let file: IPersistFile = link.cast().map_err(|e| e.to_string())?;
        file.Save(&HSTRING::from(lnk.as_os_str()), true)
            .map_err(|e| format!("saving shortcut: {e}"))?;
    }
    log::info!("created Start menu shortcut {}", lnk.display());
    Ok(())
}

struct Media {
    smtc: SystemMediaTransportControls,
    artwork: String,
}

thread_local! {
    static MEDIA: RefCell<Option<Media>> = const { RefCell::new(None) };
}

/// Attaches a media session to the main window. Main thread only.
pub fn install_media_session(window: &tauri::Window) -> Result<(), String> {
    let hwnd = HWND(window.hwnd().map_err(|e| e.to_string())?.0);
    let app = window.app_handle().clone();
    let interop = windows::core::factory::<
        SystemMediaTransportControls,
        ISystemMediaTransportControlsInterop,
    >()
    .map_err(|e| format!("SMTC interop: {e}"))?;
    // SAFETY: valid top-level HWND owned by Tauri.
    let smtc: SystemMediaTransportControls =
        unsafe { interop.GetForWindow(hwnd) }.map_err(|e| format!("SMTC for window: {e}"))?;
    let setup = || -> windows::core::Result<()> {
        smtc.SetIsPlayEnabled(true)?;
        smtc.SetIsPauseEnabled(true)?;
        smtc.SetIsNextEnabled(true)?;
        smtc.SetIsPreviousEnabled(true)?;
        smtc.SetIsEnabled(false)?; // enabled once a track is known
        smtc.ButtonPressed(&TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            let cmd = match args.ok()?.Button()? {
                SystemMediaTransportControlsButton::Play => ytm_core::Command::Play,
                SystemMediaTransportControlsButton::Pause => ytm_core::Command::Pause,
                SystemMediaTransportControlsButton::Next => ytm_core::Command::Next,
                SystemMediaTransportControlsButton::Previous => ytm_core::Command::Previous,
                _ => return Ok(()),
            };
            // Raised on a background thread: hand over to the UI thread.
            let app2 = app.clone();
            let _ = app.run_on_main_thread(move || crate::send(&app2, cmd));
            Ok(())
        }))?;
        Ok(())
    };
    setup().map_err(|e| format!("SMTC setup: {e}"))?;
    MEDIA.with(|m| {
        *m.borrow_mut() = Some(Media {
            smtc,
            artwork: String::new(),
        })
    });
    Ok(())
}

fn ticks(seconds: u32) -> TimeSpan {
    TimeSpan {
        Duration: i64::from(seconds) * 10_000_000,
    }
}

/// Mirrors the player state in the media session. Main thread only.
pub fn update_media_session(state: &ytm_core::PlayerState) {
    MEDIA.with(|m| {
        let Ok(mut m) = m.try_borrow_mut() else {
            return;
        };
        let Some(m) = m.as_mut() else {
            return;
        };
        if let Err(e) = apply(m, state) {
            log::warn!("media session update failed: {e}");
        }
    });
}

fn apply(m: &mut Media, s: &ytm_core::PlayerState) -> windows::core::Result<()> {
    let smtc = &m.smtc;
    if s.title.is_empty() {
        return smtc.SetIsEnabled(false);
    }
    smtc.SetIsEnabled(true)?;
    smtc.SetPlaybackStatus(if s.playing {
        MediaPlaybackStatus::Playing
    } else {
        MediaPlaybackStatus::Paused
    })?;
    let updater = smtc.DisplayUpdater()?;
    updater.SetType(MediaPlaybackType::Music)?;
    let music = updater.MusicProperties()?;
    music.SetTitle(&HSTRING::from(s.title.as_str()))?;
    music.SetArtist(&HSTRING::from(s.artist.as_str()))?;
    music.SetAlbumTitle(&HSTRING::from(s.album.as_str()))?;
    if m.artwork != s.artwork {
        // URL already validated (Google image hosts only); fetched by Windows.
        if s.artwork.is_empty() {
            updater.SetThumbnail(None)?;
        } else {
            let uri = Uri::CreateUri(&HSTRING::from(s.artwork.as_str()))?;
            updater.SetThumbnail(&RandomAccessStreamReference::CreateFromUri(&uri)?)?;
        }
        m.artwork.clone_from(&s.artwork);
    }
    updater.Update()?;
    if s.duration > 0 {
        let t = SystemMediaTransportControlsTimelineProperties::new()?;
        t.SetStartTime(ticks(0))?;
        t.SetMinSeekTime(ticks(0))?;
        t.SetEndTime(ticks(s.duration))?;
        t.SetMaxSeekTime(ticks(s.duration))?;
        t.SetPosition(ticks(s.position.min(s.duration)))?;
        smtc.UpdateTimelineProperties(&t)?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Global hotkeys, session lock notification, toasts and external links.
// Hotkeys and session events arrive as window messages in the same subclass
// procedure as the thumbnail buttons (`thumb_proc`).
// ---------------------------------------------------------------------------

use windows::{
    Data::Xml::Dom::XmlDocument,
    UI::Notifications::{ToastNotification, ToastNotificationManager},
    Win32::{
        System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification},
        UI::{
            Input::KeyboardAndMouse::{
                HOT_KEY_MODIFIERS, MOD_NOREPEAT, RegisterHotKey, UnregisterHotKey,
            },
            Shell::ShellExecuteW,
            WindowsAndMessaging::SW_SHOWNORMAL,
        },
    },
};

const WM_HOTKEY: u32 = 0x0312;
/// Broadcast by a second launch; the running instance shows its window.
const SHOW_MESSAGE: windows::core::PCWSTR = w!("dev.ytm-desktop.baseline.show");
static SHOW_MSG: OnceLock<u32> = OnceLock::new();

/// Single instance: a named mutex in this Windows session. When it already
/// exists, asks the running app to show its window (unless this launch is the
/// silent autostart) and returns true: the caller exits.
pub fn another_instance_running(autostart: bool) -> bool {
    use windows::Win32::{
        Foundation::{ERROR_ALREADY_EXISTS, GetLastError, LPARAM, WPARAM},
        System::Threading::CreateMutexW,
        UI::WindowsAndMessaging::{HWND_BROADCAST, PostMessageW},
    };
    // SAFETY: plain named mutex; the handle stays open for the process lifetime
    // on purpose (Windows releases it at exit).
    let created = unsafe { CreateMutexW(None, false, w!("Local\\dev.ytm-desktop.baseline")) };
    // SAFETY: reads this thread's last error, set by CreateMutexW just above.
    if created.is_err() || unsafe { GetLastError() } != ERROR_ALREADY_EXISTS {
        return false;
    }
    if !autostart {
        // SAFETY: registered message id and a broadcast post with no pointers.
        let msg = unsafe { RegisterWindowMessageW(SHOW_MESSAGE) };
        let _ = unsafe { PostMessageW(Some(HWND_BROADCAST), msg, WPARAM(0), LPARAM(0)) };
    }
    true
}
const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
const WTS_SESSION_LOCK: usize = 0x7;

static MAIN_HWND: OnceLock<isize> = OnceLock::new();

fn main_hwnd() -> Option<HWND> {
    MAIN_HWND.get().map(|h| HWND(*h as *mut core::ffi::c_void))
}

/// Session lock events for "pause when the session is locked". Main thread only.
fn register_session_events(hwnd: HWND) {
    // SAFETY: valid top-level HWND owned by Tauri.
    if let Err(e) = unsafe { WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) } {
        log::warn!("session lock events unavailable: {e}");
    }
}

/// (Re)registers the global hotkeys; returns the shortcuts Windows refused
/// (already taken by another app). Ids 1..=5 follow `Hotkeys::entries`.
pub fn register_hotkeys(keys: &ytm_core::hotkey::Hotkeys) -> Vec<String> {
    let Some(hwnd) = main_hwnd() else {
        return Vec::new();
    };
    let mut refused = Vec::new();
    for (i, (_, text)) in keys.entries().iter().enumerate() {
        let id = i as i32 + 1;
        // SAFETY: valid HWND; unregistering an unused id just fails.
        let _ = unsafe { UnregisterHotKey(Some(hwnd), id) };
        let Some(hk) = ytm_core::hotkey::parse(text) else {
            continue;
        };
        let mods = HOT_KEY_MODIFIERS(hk.modifiers) | MOD_NOREPEAT;
        // SAFETY: valid HWND and plain integer arguments.
        if let Err(e) = unsafe { RegisterHotKey(Some(hwnd), id, mods, hk.vk) } {
            log::warn!("hotkey {text} refused: {e}");
            refused.push((*text).to_owned());
        }
    }
    refused
}

fn handle_window_message(msg: u32, wparam: WPARAM) -> bool {
    let Some(app) = APP.get() else {
        return false;
    };
    let app2 = app.clone();
    // SAFETY: registering a message name is idempotent and has no pointer output.
    let show = *SHOW_MSG.get_or_init(|| unsafe { RegisterWindowMessageW(SHOW_MESSAGE) });
    if msg == show {
        let _ = app.run_on_main_thread(move || crate::show_main(&app2));
        return true;
    }
    match msg {
        WM_HOTKEY => {
            let id = wparam.0;
            let _ = app.run_on_main_thread(move || crate::features::on_hotkey(&app2, id));
            true
        }
        WM_WTSSESSION_CHANGE if wparam.0 == WTS_SESSION_LOCK => {
            let _ = app.run_on_main_thread(move || crate::features::on_session_lock(&app2));
            false // let the default procedure see it too
        }
        _ => false,
    }
}

/// Windows notification with two text lines (no sound). The app's AppUserModelID
/// and Start menu shortcut identify the sender.
pub fn notify(title: &str, body: &str) -> windows::core::Result<()> {
    let esc = |s: &str| {
        s.replace('&', "&amp;")
            .replace('<', "&lt;")
            .replace('>', "&gt;")
            .replace('"', "&quot;")
    };
    let xml = format!(
        "<toast><visual><binding template=\"ToastGeneric\"><text>{}</text><text>{}</text></binding></visual><audio silent=\"true\"/></toast>",
        esc(title),
        esc(body)
    );
    let doc = XmlDocument::new()?;
    doc.LoadXml(&HSTRING::from(xml))?;
    let toast = ToastNotification::CreateToastNotification(&doc)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?.Show(&toast)
}

/// Opens an https URL in the default browser.
pub fn open_url(url: &str) {
    // SAFETY: NUL-terminated strings that outlive the call.
    let r = unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(url),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecute returns a value > 32 on success.
    if (r.0 as isize) <= 32 {
        log::warn!("cannot open {url}");
    }
}
