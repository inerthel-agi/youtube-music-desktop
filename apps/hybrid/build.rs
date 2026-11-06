fn main() {
    // App manifest makes `player_event` a permission (`allow-player-event`) so the
    // remote origin gets that single command and nothing else.
    tauri_build::try_build(
        tauri_build::Attributes::new()
            .app_manifest(tauri_build::AppManifest::new().commands(&["player_event"])),
    )
    .expect("tauri build script failed");
}
