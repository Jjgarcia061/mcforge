//! The only surface the UI can call. Each command is a thin adapter over a core module.

use serde::Serialize;
use tauri::State;
use tauri::ipc::Channel;

use crate::catalog::{BuiltinModule, Catalog, Server, TestReport, Values};
use crate::database;
use crate::error::{Error, Result};
use crate::hardware::Hardware;
use crate::providers::ollama::{self, OllamaStatus, PullProgress};
use crate::providers::{self, Provider};
use crate::recommend::{self, Recommendation};
use crate::secrets;

pub struct AppState {
    pub catalog: Catalog,
    pub hardware: Hardware,
    pub http: reqwest::Client,
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
