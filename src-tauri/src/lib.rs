mod atomic;
mod catalog;
mod chat;
mod commands;
mod database;
mod error;
mod hardware;
mod library;
mod policy;
mod providers;
mod recommend;
mod reports;
mod secrets;
mod setup;
mod sources;

use std::collections::HashMap;
use std::sync::{Mutex, RwLock};

use tauri::Manager;

use crate::atomic::Stored;
use crate::catalog::Catalog;
use crate::chat::store::Chats;
use crate::commands::{AppState, Paths};
use crate::library::Library;
use crate::reports::Templates;
use crate::reports::focus::Focuses;
use crate::reports::history::History;

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
        commands::load_setup,
        commands::save_setup,
        commands::list_templates,
        commands::inspect_source,
        commands::generate_report,
        commands::cancel_report,
        commands::list_focuses,
        commands::save_focus,
        commands::delete_focus,
        commands::list_reports,
        commands::read_report,
        commands::open_file,
        commands::library_status,
        commands::set_library_folders,
        commands::update_library,
        commands::cancel_library_update,
        commands::ask_library,
        commands::library_graph,
        commands::list_chats,
        commands::load_chat,
        commands::delete_chat,
        commands::send_message,
        commands::answer_approval,
        commands::stop_chat,
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
            let path = app.path();
            let resources = path.resource_dir()?;
            let catalog = Catalog::load(&resources.join("catalog"))?;
            let paths = Paths {
                setup: path.app_config_dir()?.join("setup.json"),
                cache: path.app_cache_dir()?,
                history: path.app_data_dir()?.join("reports.json"),
                focuses: path.app_data_dir()?.join("focus.json"),
                library: path.app_data_dir()?.join("library.db"),
                chats: path.app_data_dir()?.join("chats"),
                activity: path.app_data_dir()?.join("activity.json"),
                default_reports: path
                    .document_dir()
                    .or_else(|_| path.home_dir())?
                    .join("MCForge"),
            };
            app.manage(AppState {
                setup: RwLock::new(setup::load(&paths.setup, &catalog)?),
                history: History::load(paths.history.clone()),
                focuses: Focuses::load(paths.focuses.clone()),
                chats: Chats::new(paths.chats.clone()),
                activity: Stored::load(paths.activity.clone()),
                library: tauri::async_runtime::block_on(Library::open(&paths.library))?,
                templates: Templates::load(&resources.join("templates/reports"))?,
                catalog,
                hardware: hardware::detect()?,
                http: reqwest::Client::builder()
                    .user_agent(concat!("MCForge/", env!("CARGO_PKG_VERSION")))
                    .build()?,
                paths,
                running: Mutex::new(None),
                indexing: Mutex::new(None),
                approvals: Mutex::new(HashMap::new()),
                chatting: Mutex::new(None),
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running MCForge");
}
