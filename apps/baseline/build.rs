fn main() {
    // The app manifest turns each command into a permission (`allow-<name>`), so
    // every capability file grants exactly the commands its window needs:
    // music.youtube.com gets player_event + window_control, the local pages the rest.
    tauri_build::try_build(tauri_build::Attributes::new().app_manifest(
        tauri_build::AppManifest::new().commands(&[
            "player_event",
            "mini_command",
            "window_control",
            "get_settings",
            "update_settings",
            "audio_preset",
            "restart_app",
            "eq_preset",
            "open_url",
            "get_sleep_timer",
            "set_sleep_timer",
        ]),
    ))
    .expect("tauri build script failed");
}
