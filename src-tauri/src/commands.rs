//! The only surface the UI can call. Each command is a thin adapter over a core module.

use std::collections::HashMap;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};

use futures_util::future::{AbortHandle, Abortable, BoxFuture};
use serde::{Deserialize, Serialize};
use tauri::State;
use tauri::ipc::Channel;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

use crate::atomic::Stored;
use crate::catalog::{BuiltinModule, Catalog, Server, TestReport, Values};
use crate::chat::store::{Chat, ChatSummary, Chats};
use crate::chat::tools::{Attachment, Builtin, ReportTool};
use crate::chat::{self, Activity, Approval, ChatEvent, Host};
use crate::database::{self, Driver};
use crate::error::{Error, Result};
use crate::hardware::Hardware;
use crate::library::{
    Answer, Embedder, Graph, Library, LibraryStatus, OllamaEmbedder, Progress, UpdateSummary,
};
use crate::policy::Permission;
use crate::providers::chat::EngineModel;
use crate::providers::ollama::{self, OllamaStatus, PullProgress};
use crate::providers::{self, Provider};
use crate::recommend::{self, Recommendation};
use crate::reports::focus::{Focus, Focuses, SavedFocuses};
use crate::reports::history::{History, ReportEntry};
use crate::reports::{
    self, Ai, NumberFormat, ReportOutcome, ReportRequest, ReportStage, Snippet, TemplateInfo,
    Templates,
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
    pub history: History,
    pub focuses: Focuses,
    pub library: Library,
    /// The report being generated, so `cancel_report` can stop it.
    pub running: Mutex<Option<AbortHandle>>,
    /// The library update in progress, so `cancel_library_update` can stop it.
    pub indexing: Mutex<Option<AbortHandle>>,
    pub chats: Chats,
    /// Every tool call made from a conversation, newest last.
    pub activity: Stored<Vec<Activity>>,
    /// Tool calls waiting for the user's answer, by call id.
    pub approvals: Mutex<HashMap<String, oneshot::Sender<Approval>>>,
    /// The conversation turn in progress, so `stop_chat` can stop it.
    pub chatting: Mutex<Option<AbortHandle>>,
}

pub struct Paths {
    pub setup: PathBuf,
    pub cache: PathBuf,
    pub history: PathBuf,
    pub focuses: PathBuf,
    pub library: PathBuf,
    pub chats: PathBuf,
    pub activity: PathBuf,
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
    // Imports and reports are only moved into place once complete, so stopping anywhere is safe.
    cancellable(&state.running, report(&state, &request, &on_stage)).await
}

/// Stops the report being generated, which then fails as cancelled.
#[tauri::command]
#[specta::specta]
pub fn cancel_report(state: State<'_, AppState>) {
    cancel(&state.running);
}

/// Runs `work` so that `cancel(slot)` can stop it from another command: the
/// future is dropped wherever it is, which also closes any open model request.
async fn cancellable<T>(
    slot: &Mutex<Option<AbortHandle>>,
    work: impl Future<Output = Result<T>>,
) -> Result<T> {
    let (handle, registration) = AbortHandle::new_pair();
    if let Ok(mut current) = slot.lock() {
        *current = Some(handle);
    }
    Abortable::new(work, registration)
        .await
        .unwrap_or_else(|_| Err(Error::Io("cancelled".into())))
}

fn cancel(slot: &Mutex<Option<AbortHandle>>) {
    if let Some(handle) = slot.lock().ok().and_then(|mut h| h.take()) {
        handle.abort();
    }
}

async fn report(
    state: &AppState,
    request: &ReportRequest,
    on_stage: &Channel<ReportStage>,
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

    let context = if ai.is_some() && request.use_library {
        library_context(state, request, &source.summary).await
    } else {
        Vec::new()
    };
    let outcome = reports::generate(
        &state.templates,
        &source,
        ai,
        request,
        &context,
        &out_dir,
        stage,
    )
    .await?;
    // The report is written either way; files that cannot be saved only forget it on restart.
    if let Some(focus) = &request.focus {
        let _ = state.focuses.remember(&focus.id);
    }
    let _ = state
        .history
        .add(ReportEntry::new(&outcome, source.summary.name()));
    Ok(outcome)
}

/// Whether the library can search by meaning: Ollama must be running and have the model.
#[derive(Debug, Clone, Copy, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum SearchModel {
    Ready,
    Missing,
    Offline,
}

#[derive(Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryOverview {
    pub status: LibraryStatus,
    pub search_model: SearchModel,
}

/// The library's embedding model, if Ollama can run it now.
async fn embedder(state: &AppState) -> (SearchModel, Option<OllamaEmbedder>) {
    let Some(model) = state.catalog.models.embedding.first() else {
        return (SearchModel::Offline, None);
    };
    match ollama::status(&state.http).await {
        Ok(OllamaStatus::Running { models, .. }) => {
            let installed = models
                .iter()
                .any(|m| m == &model.id || m.strip_suffix(":latest") == Some(&model.id));
            if !installed {
                return (SearchModel::Missing, None);
            }
            let embedder = OllamaEmbedder {
                http: state.http.clone(),
                model: model.id.clone(),
                query_prefix: model.query_prefix.clone(),
            };
            (SearchModel::Ready, Some(embedder))
        }
        _ => (SearchModel::Offline, None),
    }
}

#[tauri::command]
#[specta::specta]
pub async fn library_status(state: State<'_, AppState>) -> Result<LibraryOverview> {
    let (search_model, _) = embedder(&state).await;
    Ok(LibraryOverview {
        status: state.library.status().await?,
        search_model,
    })
}

#[tauri::command]
#[specta::specta]
pub async fn set_library_folders(
    state: State<'_, AppState>,
    folders: Vec<String>,
) -> Result<LibraryOverview> {
    state.library.set_folders(&folders).await?;
    library_status(state).await
}

/// Indexes what changed in the library folders since the last update.
#[tauri::command]
#[specta::specta]
pub async fn update_library(
    state: State<'_, AppState>,
    on_progress: Channel<Progress>,
) -> Result<UpdateSummary> {
    let (_, embedder) = embedder(&state).await;
    let work = state.library.update(
        embedder
            .as_ref()
            .map(|e| e as &dyn crate::library::Embedder),
        |p| drop(on_progress.send(p)),
    );
    cancellable(&state.indexing, work).await
}

#[tauri::command]
#[specta::specta]
pub fn cancel_library_update(state: State<'_, AppState>) {
    cancel(&state.indexing);
}

/// Answers from the library's documents, citing the passages used.
#[tauri::command]
#[specta::specta]
pub async fn ask_library(
    state: State<'_, AppState>,
    question: String,
    language: String,
) -> Result<Answer> {
    let engine = state
        .setup
        .read()
        .ok()
        .and_then(|s| s.as_ref().map(|s| s.engine.clone()))
        .ok_or_else(|| Error::Provider("Set up an AI engine first.".into()))?;
    let model = EngineModel::new(state.http.clone(), engine)?;
    let (_, embedder) = embedder(&state).await;
    state
        .library
        .ask(
            &model,
            embedder
                .as_ref()
                .map(|e| e as &dyn crate::library::Embedder),
            &question,
            reports::language_name(&language),
        )
        .await
}

#[tauri::command]
#[specta::specta]
pub async fn library_graph(state: State<'_, AppState>) -> Result<Graph> {
    state.library.graph().await
}

/// Library passages that bear on a report, found by its question (or the
/// file's name and columns when there is none) and its focus.
async fn library_context(
    state: &AppState,
    request: &ReportRequest,
    summary: &SourceSummary,
) -> Vec<Snippet> {
    const PASSAGES: usize = 4;
    const MAX_CHARS: usize = 900;
    let mut query = request.question.clone().unwrap_or_default();
    if query.trim().is_empty() {
        query = std::iter::once(summary.file.as_str())
            .chain(summary.columns.iter().map(|c| c.label.as_str()))
            .collect::<Vec<_>>()
            .join(" ");
    }
    if let Some(focus) = &request.focus {
        query = format!("{query} {}", focus.name);
    }
    let (_, embedder) = embedder(state).await;
    // Context is a bonus: a library that cannot be searched does not stop the report.
    state
        .library
        .search(
            embedder.as_ref().map(|e| e as &dyn Embedder),
            &query,
            PASSAGES,
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|p| Snippet {
            title: p.title,
            text: p.text.chars().take(MAX_CHARS).collect(),
        })
        .collect()
}

/// The user's own report focuses and the one used last.
#[tauri::command]
#[specta::specta]
pub fn list_focuses(state: State<'_, AppState>) -> SavedFocuses {
    state.focuses.saved()
}

/// Creates (empty id) or updates a custom focus and returns it with its id.
#[tauri::command]
#[specta::specta]
pub fn save_focus(state: State<'_, AppState>, focus: Focus) -> Result<Focus> {
    state.focuses.save(focus)
}

#[tauri::command]
#[specta::specta]
pub fn delete_focus(state: State<'_, AppState>, id: String) -> Result<()> {
    state.focuses.delete(&id)
}

/// Reports generated on this computer whose files still exist, newest first.
#[tauri::command]
#[specta::specta]
pub fn list_reports(state: State<'_, AppState>) -> Vec<ReportEntry> {
    state.history.list()
}

/// A generated report's HTML, for the in-app viewer.
#[tauri::command]
#[specta::specta]
pub fn read_report(state: State<'_, AppState>, path: String) -> Result<String> {
    if !state.history.contains(Path::new(&path)) {
        return Err(unknown_file());
    }
    Ok(std::fs::read_to_string(&path)?)
}

/// Opens a report MCForge generated or a library file with its default app, or
/// shows it in its folder.
#[tauri::command]
#[specta::specta]
pub fn open_file(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    path: String,
    reveal: bool,
) -> Result<()> {
    if !state.history.contains(Path::new(&path)) && !state.library.contains(&path) {
        return Err(unknown_file());
    }
    let opener = app.opener();
    let opened = if reveal {
        opener.reveal_item_in_dir(&path)
    } else {
        opener.open_path(&path, None::<&str>)
    };
    opened.map_err(|e| Error::Io(e.to_string()))
}

/// The UI can only reach files MCForge wrote or indexed, never arbitrary paths.
fn unknown_file() -> Error {
    Error::Io("MCForge only opens its reports and the files in your library".into())
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

/// Entries kept in the activity log.
const MAX_ACTIVITY: usize = 1000;

#[derive(Deserialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ChatContext {
    pub language: String,
    pub number_format: NumberFormat,
    /// Preset report focus names in the user's language, by id.
    pub focus_names: HashMap<String, String>,
}

#[tauri::command]
#[specta::specta]
pub fn list_chats(state: State<'_, AppState>) -> Vec<ChatSummary> {
    state.chats.list()
}

#[tauri::command]
#[specta::specta]
pub fn load_chat(state: State<'_, AppState>, id: String) -> Result<Chat> {
    state.chats.load(&id)
}

#[tauri::command]
#[specta::specta]
pub fn delete_chat(state: State<'_, AppState>, id: String) -> Result<()> {
    state.chats.delete(&id)
}

/// Asks the engine in a conversation (a new one without `chat_id`) and streams
/// the answer. The conversation is saved before and after, also when stopped.
#[tauri::command]
#[specta::specta]
pub async fn send_message(
    state: State<'_, AppState>,
    chat_id: Option<String>,
    text: String,
    attachments: Vec<Attachment>,
    context: ChatContext,
    on_event: Channel<ChatEvent>,
) -> Result<Chat> {
    let mut chat = match &chat_id {
        Some(id) => state.chats.load(id)?,
        None => Chat::new(&text),
    };
    for a in attachments {
        if !chat
            .attachments
            .iter()
            .any(|x| x.path == a.path && x.sheet == a.sheet)
        {
            chat.attachments.push(a);
        }
    }
    chat.ask(&text);
    state.chats.save(&chat)?;
    if let Ok(mut pending) = state.approvals.lock() {
        pending.clear();
    }
    let outcome = cancellable(
        &state.chatting,
        converse(&state, &mut chat, &context, &on_event),
    )
    .await;
    chat::repair(&mut chat.messages);
    chat.updated = chrono::Local::now().to_rfc3339();
    state.chats.save(&chat)?;
    outcome.map(|()| chat)
}

/// The user's answer to a tool call the rules ask about.
#[tauri::command]
#[specta::specta]
pub fn answer_approval(state: State<'_, AppState>, id: String, approval: Approval) {
    if let Some(waiting) = state.approvals.lock().ok().and_then(|mut p| p.remove(&id)) {
        let _ = waiting.send(approval);
    }
}

#[tauri::command]
#[specta::specta]
pub fn stop_chat(state: State<'_, AppState>) {
    cancel(&state.chatting);
}

/// One turn with the tools the user set up.
async fn converse(
    state: &AppState,
    chat: &mut Chat,
    context: &ChatContext,
    on_event: &Channel<ChatEvent>,
) -> Result<()> {
    let setup = state
        .setup
        .read()
        .ok()
        .and_then(|s| s.clone())
        .ok_or_else(|| Error::Provider("Set up an AI engine first.".into()))?;
    let model = EngineModel::new(state.http.clone(), setup.engine.clone())?;
    let label = engine_label(&state.catalog, &setup.engine);
    let (_, embedder) = embedder(state).await;
    let library = (state.library.documents().await? > 0).then_some((
        &state.library,
        embedder.as_ref().map(|e| e as &dyn Embedder),
    ));
    let selected = |id: &str| setup.connectors.iter().any(|c| c == id);
    let database = selected("database")
        .then(|| {
            let driver: Driver = setup.text("database", "driver")?.parse().ok()?;
            Some((driver, setup.text("database", "url")?.to_owned()))
        })
        .flatten();
    let reports = selected("reports").then(|| {
        let presets = state.catalog.focuses.iter().map(|p| Focus {
            id: p.id.clone(),
            name: context
                .focus_names
                .get(&p.id)
                .cloned()
                .unwrap_or_else(|| p.id.clone()),
            instructions: p.instructions.clone(),
        });
        ReportTool {
            templates: &state.templates,
            history: &state.history,
            cache: &state.paths.cache,
            out_dir: setup
                .text("reports", "output_dir")
                .map_or_else(|| state.paths.default_reports.clone(), PathBuf::from),
            model: &model,
            label: &label,
            attachments: &chat.attachments,
            focuses: presets.chain(state.focuses.saved().custom).collect(),
            language: context.language.clone(),
            number_format: context.number_format.clone(),
        }
    });
    let tools = Builtin::new(
        &state.catalog,
        setup.permissions.clone(),
        library,
        database,
        reports,
    );
    let host = StateHost {
        state,
        events: on_event,
    };
    let system = chat::system_prompt(
        reports::language_name(&context.language),
        &chrono::Local::now().format("%Y-%m-%d").to_string(),
        &chat.attachments,
    );
    // A local model's context is small; an API model's is large.
    let budget = if matches!(setup.engine, Engine::Local { .. }) {
        40_000
    } else {
        200_000
    };
    chat::run_turn(&model, &tools, &host, &system, budget, &mut chat.messages).await
}

struct StateHost<'a> {
    state: &'a AppState,
    events: &'a Channel<ChatEvent>,
}

impl StateHost<'_> {
    /// "Allow always": the rule is saved like any other set in the setup.
    fn allow_always(&self, name: &str) {
        let Some((connector, tool)) = name.split_once('_') else {
            return;
        };
        let Ok(mut current) = self.state.setup.write() else {
            return;
        };
        let Some(setup) = current.as_mut() else {
            return;
        };
        setup
            .permissions
            .entry(connector.to_owned())
            .or_default()
            .insert(tool.to_owned(), Permission::Allow);
        // The rule holds for this session even if it cannot be saved.
        let _ = setup::save(&self.state.paths.setup, setup, &self.state.catalog);
    }
}

impl Host for StateHost<'_> {
    fn emit(&self, event: ChatEvent) {
        // A closed channel only means the window went away; the turn goes on.
        drop(self.events.send(event));
    }

    fn approve<'a>(&'a self, id: &'a str, name: &'a str) -> BoxFuture<'a, Approval> {
        let (send, answer) = oneshot::channel();
        if let Ok(mut pending) = self.state.approvals.lock() {
            pending.insert(id.to_owned(), send);
        }
        Box::pin(async move {
            let approval = answer.await.unwrap_or(Approval::Reject);
            if approval == Approval::Always {
                self.allow_always(name);
            }
            approval
        })
    }

    fn record(&self, activity: Activity) {
        let _ = self.state.activity.update(|log| {
            log.push(activity);
            let extra = log.len().saturating_sub(MAX_ACTIVITY);
            log.drain(..extra);
        });
    }
}
