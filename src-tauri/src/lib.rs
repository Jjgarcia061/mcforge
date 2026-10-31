mod catalog;
mod commands;
mod database;
mod error;
mod hardware;
mod policy;
mod providers;
mod recommend;
mod secrets;

use tauri::Manager;

use crate::catalog::Catalog;
use crate::commands::AppState;

/// Typed command registry; also generates `src/lib/bindings.ts` for the UI.
pub fn specta_builder() -> tauri_specta::Builder<tauri::Wry> {
    tauri_specta::Builder::<tauri::Wry>::new().commands(tauri_specta::collect_commands![
        commands::system_report,
        commands::get_catalog,
        commands::ollama_status,
        commands::pull_model,
        commands::connect_provider,
        commands::saved_provider_models,
        commands::test_connection,
    ])
}

pub const BINDINGS_PATH: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../src/lib/bindings.ts");

pub fn export_bindings(builder: &tauri_specta::Builder<tauri::Wry>) {
    builder
        .export(specta_typescript::Typescript::default(), BINDINGS_PATH)
        .expect("failed to export TypeScript bindings");
}

pub fn run() {
    let builder = specta_builder();
    #[cfg(debug_assertions)]
    export_bindings(&builder);

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(builder.invoke_handler())
        .setup(|app| {
            let catalog = Catalog::load(&app.path().resource_dir()?.join("catalog"))?;
            app.manage(AppState {
                catalog,
                hardware: hardware::detect()?,
                http: reqwest::Client::builder()
                    .user_agent(concat!("MCForge/", env!("CARGO_PKG_VERSION")))
                    .build()?,
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running MCForge");
}
