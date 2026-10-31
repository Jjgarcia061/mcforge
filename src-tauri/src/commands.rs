//! The only surface the UI can call. Each command is a thin adapter over a core module.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;
use tauri_plugin_opener::OpenerExt;

use crate::catalog::{BuiltinModule, Catalog, Server, TestReport, Values};
use crate::database;
use crate::error::{Error, Result};
use crate::hardware::Hardware;
use crate::providers::chat::EngineModel;
use crate::providers::ollama::{self, OllamaStatus, PullProgress};
use crate::providers::{self, Provider};
use crate::recommend::{self, Recommendation};
use crate::reports::{
    self, Ai, ReportOutcome, ReportRequest, ReportStage, TemplateInfo, Templates,
};
use crate::secrets;
use crate::setup::{self, Engine, Setup};
use crate::sources::{FileSource, SourceSummary};

pub struct AppState {
    pub catalog: Catalog,
    pub templates: Templates,
    pub hardware: Hardware,
    pub http: reqwest::Client,
    pub paths: Paths,
    pub setup: RwLock<Option<Setup>>,
    /// Reports generated in this session; the only files `open_report` will open.
    pub reports: Mutex<HashSet<PathBuf>>,
}

pub struct Paths {
    pub setup: PathBuf,
    pub cache: PathBuf,
    /// Where reports go unless the `reports` connector says otherwise.
    pub default_reports: PathBuf,
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct SystemReport {
    pub hardware: Hardware,
    pub recommendations: Vec<Recommendation>,
}

#[tauri::command]
#[specta::specta]
pub fn system_report(state: State<'_, AppState>) -> SystemReport {
    SystemReport {
        recommendations: recommend::rank(&state.hardware, &state.catalog.models.local),
        hardware: state.hardware.clone(),
    }
}

#[tauri::command]
#[specta::specta]
pub fn get_catalog(state: State<'_, AppState>) -> Catalog {
    state.catalog.clone()
}

#[tauri::command]
#[specta::specta]
pub async fn ollama_status(state: State<'_, AppState>) -> Result<OllamaStatus> {
    ollama::status(&state.http).await
}

#[tauri::command]
#[specta::specta]
pub async fn pull_model(
    state: State<'_, AppState>,
    model: String,
    on_progress: Channel<PullProgress>,
) -> Result<()> {
    // A closed channel only means the window went away; the download itself goes on.
    ollama::pull(&state.http, &model, |p| drop(on_progress.send(p))).await
}

/// Validates the key against the provider and stores it only if it works.
#[tauri::command]
#[specta::specta]
pub async fn connect_provider(
    state: State<'_, AppState>,
    provider: Provider,
    api_key: String,
) -> Result<Vec<String>> {
    let models = providers::list_models(&state.http, provider, &api_key).await?;
    secrets::set(provider.key_account(), api_key.trim())?;
    Ok(models)
}

/// Models available with the key already in the keychain, or `None` if there is no key yet.
#[tauri::command]
#[specta::specta]
pub async fn saved_provider_models(
    state: State<'_, AppState>,
    provider: Provider,
) -> Result<Option<Vec<String>>> {
    match secrets::get(provider.key_account())? {
        Some(key) => Ok(Some(
            providers::list_models(&state.http, provider, &key).await?,
        )),
        None => Ok(None),
    }
}

/// Checks a connector's configuration before it is saved.
#[tauri::command]
#[specta::specta]
pub async fn test_connection(
    state: State<'_, AppState>,
    connector_id: String,
    values: Values,
) -> Result<TestReport> {
    match state.catalog.connector(&connector_id)?.server {
        Server::Builtin {
            module: BuiltinModule::Database,
        } => database::test(&values).await,
        _ => Err(Error::Provider(format!(
            "`{connector_id}` cannot be tested yet"
        ))),
    }
}

/// The saved setup, or `None` on first run.
#[tauri::command]
#[specta::specta]
pub fn load_setup(state: State<'_, AppState>) -> Option<Setup> {
    state.setup.read().ok()?.clone()
}

#[tauri::command]
#[specta::specta]
pub fn save_setup(state: State<'_, AppState>, setup: Setup) -> Result<()> {
    setup::save(&state.paths.setup, &setup, &state.catalog)?;
    if let Ok(mut current) = state.setup.write() {
        *current = Some(setup);
    }
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn list_templates(state: State<'_, AppState>, language: String) -> Vec<TemplateInfo> {
    state.templates.list(&language)
}

/// Imports a CSV file or a workbook sheet (or reuses its cached import) and describes its columns.
#[tauri::command]
#[specta::specta]
pub async fn inspect_source(
    state: State<'_, AppState>,
    path: String,
    sheet: Option<String>,
) -> Result<SourceSummary> {
    Ok(
        FileSource::import(Path::new(&path), sheet.as_deref(), &state.paths.cache)
            .await?
            .summary,
    )
}

#[tauri::command]
#[specta::specta]
pub async fn generate_report(
    state: State<'_, AppState>,
    request: ReportRequest,
    on_stage: Channel<ReportStage>,
) -> Result<ReportOutcome> {
    let stage = |s| drop(on_stage.send(s));
    stage(ReportStage::Reading);
    let source = FileSource::import(
        Path::new(&request.file),
        request.sheet.as_deref(),
        &state.paths.cache,
    )
    .await?;

    let saved = state.setup.read().ok().and_then(|s| s.clone());
    let out_dir = saved
        .as_ref()
        .and_then(|s| s.text("reports", "output_dir"))
        .map_or_else(|| state.paths.default_reports.clone(), PathBuf::from);
    let engine = saved.map(|s| s.engine).filter(|_| request.use_ai);
    let model = engine
        .as_ref()
        .map(|e| EngineModel::new(state.http.clone(), e.clone()))
        .transpose()?;
    let label = engine
        .as_ref()
        .map(|e| engine_label(&state.catalog, e))
        .unwrap_or_default();
    let ai = model.as_ref().map(|m| Ai {
        model: m,
        label: &label,
    });

    let outcome =
        reports::generate(&state.templates, &source, ai, &request, &out_dir, stage).await?;
    if let Ok(mut reports) = state.reports.lock() {
        reports.insert(PathBuf::from(&outcome.path));
    }
    Ok(outcome)
}

/// Opens a report generated in this session, or shows it in its folder.
#[tauri::command]
#[specta::specta]
pub fn open_report(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    reveal: bool,
) -> Result<()> {
    let known = state
        .reports
        .lock()
        .is_ok_and(|r| r.contains(Path::new(&path)));
    if !known {
        return Err(Error::Report(
            "that file was not generated by MCForge".into(),
        ));
    }
    let opener = app.opener();
    let opened = if reveal {
        opener.reveal_item_in_dir(&path)
    } else {
        opener.open_path(&path, None::<&str>)
    };
    opened.map_err(|e| Error::Io(e.to_string()))
}

fn engine_label(catalog: &Catalog, engine: &Engine) -> String {
    match engine {
        Engine::Local { model } => model.clone(),
        Engine::Api { model, .. } => catalog
            .models
            .api
            .iter()
            .find(|m| &m.id == model)
            .map_or_else(|| model.clone(), |m| m.name.clone()),
    }
}
