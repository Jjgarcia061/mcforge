//! The document library: folders the user picks, indexed so the AI can answer
//! questions about them with citations and use them as context for reports,
//! and drawn as a map of related files. MCForge only ever reads those folders;
//! the index lives in the app's data folder and updates incrementally.

pub mod extract;
pub mod graph;
mod index;
mod search;

use std::collections::{HashMap, HashSet};
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::UNIX_EPOCH;

use futures_util::future::BoxFuture;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

pub use self::graph::Graph;
pub use self::index::Passage;
use self::index::{Entry, Index, Stored};
use self::search::Vectors;
use crate::error::{Error, Result};
use crate::providers::chat::Model;
use crate::providers::ollama;

/// Passages embedded per request.
const EMBED_BATCH: u32 = 32;
/// Passages a question is answered from.
const ASK_PASSAGES: usize = 8;
const EXCERPT_CHARS: usize = 320;

/// Turns text into vectors that are close when the meaning is.
pub trait Embedder: Send + Sync {
    /// Stored with the vectors, so switching models rebuilds them.
    fn id(&self) -> &str;
    /// `query` marks a search question, which some models encode differently from passages.
    fn embed<'a>(
        &'a self,
        texts: &'a [String],
        query: bool,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>>>;
}

pub struct OllamaEmbedder {
    pub http: reqwest::Client,
    pub model: String,
    pub query_prefix: String,
}

impl Embedder for OllamaEmbedder {
    fn id(&self) -> &str {
        &self.model
    }

    fn embed<'a>(
        &'a self,
        texts: &'a [String],
        query: bool,
    ) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
        Box::pin(async move {
            if query && !self.query_prefix.is_empty() {
                let prefixed: Vec<String> = texts
                    .iter()
                    .map(|t| format!("{}{t}", self.query_prefix))
                    .collect();
                ollama::embed(&self.http, &self.model, &prefixed).await
            } else {
                ollama::embed(&self.http, &self.model, texts).await
            }
        })
    }
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct LibraryStatus {
    pub folders: Vec<String>,
    pub files: u32,
    pub chunks: u32,
    /// Files with no readable text (scanned PDFs, damaged files).
    pub failed: u32,
    /// RFC 3339, when the last update finished.
    pub updated: Option<String>,
    pub pending: Pending,
}

/// What an update would do now.
#[derive(Debug, Clone, Default, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Pending {
    pub added: u32,
    pub changed: u32,
    pub removed: u32,
    /// Passages still without a vector (indexed while the embedding model was unavailable).
    pub unembedded: u32,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "phase", rename_all = "camelCase")]
pub enum Progress {
    Scanning,
    Reading { done: u32, total: u32, file: String },
    Embedding { done: u32, total: u32 },
    Mapping,
}

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSummary {
    pub added: u32,
    pub changed: u32,
    pub removed: u32,
    /// Files that could not be read, with the reason.
    pub failed: Vec<FailedFile>,
    /// Why search by meaning is not up to date, if it is not.
    pub embed_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct FailedFile {
    pub path: String,
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, specta::Type)]
pub struct Answer {
    /// Cites sources as `[n]`; empty when nothing in the library matched.
    pub text: String,
    pub sources: Vec<Source>,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
pub struct Source {
    pub n: u32,
    pub title: String,
    pub path: String,
    pub excerpt: String,
}

pub struct Library {
    index: Index,
    /// Paths of indexed files: the library files `open_file` may open.
    known: RwLock<HashSet<String>>,
    vectors: Mutex<Option<Arc<Vectors>>>,
}

/// A file found on disk.
struct Found {
    path: String,
    size: i64,
    modified: i64,
}

impl Library {
    pub async fn open(path: &Path) -> Result<Self> {
        let index = Index::open(path).await?;
        let known = index.indexed_paths().await?.into_iter().collect();
        Ok(Self {
            index,
            known: RwLock::new(known),
            vectors: Mutex::new(None),
        })
    }

    pub fn contains(&self, path: &str) -> bool {
        self.known.read().is_ok_and(|k| k.contains(path))
    }

    pub async fn set_folders(&self, folders: &[String]) -> Result<()> {
        let mut clean: Vec<String> = folders
            .iter()
            .map(|f| f.trim().to_owned())
            .filter(|f| !f.is_empty())
            .collect();
        clean.sort();
        clean.dedup();
        self.index.set_folders(&clean).await
    }

    pub async fn status(&self) -> Result<LibraryStatus> {
        let folders = self.index.folders().await?;
        let found = blocking({
            let folders = folders.clone();
            move || Ok(scan(&folders))
        })
        .await?;
        let stored = self.index.files().await?;
        let (added, changed, removed) = diff(&found, &stored);
        let counts = self.index.counts().await?;
        Ok(LibraryStatus {
            folders,
            files: counts.files,
            chunks: counts.chunks,
            failed: counts.failed,
            updated: self.index.meta("updated").await?,
            pending: Pending {
                added: added.len() as u32,
                changed: changed.len() as u32,
                removed: removed.len() as u32,
                unembedded: counts.unembedded,
            },
        })
    }

    /// Brings the index in line with the folders: reads new and changed
    /// files, drops removed ones, embeds what has no vector yet and redraws the
    /// map. Safe to cancel at any point; the next update picks up from there.
    pub async fn update(
        &self,
        embedder: Option<&dyn Embedder>,
        on_progress: impl Fn(Progress) + Send + Sync,
    ) -> Result<UpdateSummary> {
        self.forget_vectors();
        on_progress(Progress::Scanning);
        let folders = self.index.folders().await?;
        let found = blocking(move || Ok(scan(&folders))).await?;
        let stored = self.index.files().await?;
        let (added, changed, removed) = diff(&found, &stored);
        let mut summary = UpdateSummary {
            added: added.len() as u32,
            changed: changed.len() as u32,
            removed: removed.len() as u32,
            ..UpdateSummary::default()
        };
        self.index
            .remove(&removed.iter().map(|s| s.id).collect::<Vec<_>>())
            .await?;

        let stored_by_path: HashMap<&str, &Stored> =
            stored.iter().map(|s| (s.path.as_str(), s)).collect();
        let work: Vec<&Found> = added.iter().chain(&changed).copied().collect();
        for (i, file) in work.iter().enumerate() {
            on_progress(Progress::Reading {
                done: i as u32,
                total: work.len() as u32,
                file: file_name(&file.path),
            });
            let old = stored_by_path.get(file.path.as_str());
            if let Some(error) = self.reindex(file, old.copied()).await? {
                summary.failed.push(FailedFile {
                    path: file.path.clone(),
                    error,
                });
            }
        }

        if let Some(embedder) = embedder {
            summary.embed_error = self.embed_missing(embedder, &on_progress).await?;
        }

        on_progress(Progress::Mapping);
        let files = self.index.graph_files().await?;
        let graph = blocking(move || Ok(graph::build(&files))).await?;
        self.index
            .set_meta("graph", &serde_json::to_string(&graph).unwrap_or_default())
            .await?;
        self.index
            .set_meta("updated", &chrono::Local::now().to_rfc3339())
            .await?;
        let paths = self.index.indexed_paths().await?;
        if let Ok(mut known) = self.known.write() {
            *known = paths.into_iter().collect();
        }
        Ok(summary)
    }

    /// Indexes one file; returns why it has no text, if it has none.
    async fn reindex(&self, file: &Found, old: Option<&Stored>) -> Result<Option<String>> {
        let path = file.path.clone();
        let old_hash = old.map(|o| o.hash.clone());
        let read = blocking(move || {
            let bytes = std::fs::read(&path)?;
            let hash = format!("{:x}", Sha256::digest(&bytes));
            // Same content under a new date: the passages stay as they are.
            let document =
                (old_hash.as_ref() != Some(&hash)).then(|| extract::read(Path::new(&path), &bytes));
            Ok((hash, document))
        })
        .await;
        let Ok((hash, document)) = read else {
            // Gone or unreadable since the scan; the next update sees it as removed.
            return Ok(None);
        };
        let Some(document) = document else {
            if let Some(old) = old {
                self.index.touch(old.id, file.size, file.modified).await?;
            }
            return Ok(None);
        };
        let p = Path::new(&file.path);
        let title = p
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let kind = p
            .extension()
            .map(|e| e.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        let (chunks, links, error) = match document {
            Ok(doc) => (extract::chunks(&doc.text), doc.links, None),
            Err(e) => (Vec::new(), Vec::new(), Some(e.to_string())),
        };
        self.index
            .put(&Entry {
                path: &file.path,
                title: &title,
                kind: &kind,
                size: file.size,
                modified: file.modified,
                hash: &hash,
                links: &links,
                error: error.as_deref(),
                chunks: &chunks,
            })
            .await?;
        Ok(error)
    }

    /// Embeds every passage without a vector; a new model re-embeds everything.
    async fn embed_missing(
        &self,
        embedder: &dyn Embedder,
        on_progress: &(impl Fn(Progress) + Send + Sync),
    ) -> Result<Option<String>> {
        if self.index.meta("embedder").await?.as_deref() != Some(embedder.id()) {
            self.index.clear_embeddings().await?;
            self.index.set_meta("embedder", embedder.id()).await?;
        }
        let total = self.index.counts().await?.unembedded;
        let mut done = 0;
        loop {
            let batch = self.index.unembedded(EMBED_BATCH).await?;
            if batch.is_empty() {
                return Ok(None);
            }
            on_progress(Progress::Embedding { done, total });
            let texts: Vec<String> = batch.iter().map(|(_, t)| t.clone()).collect();
            match embedder.embed(&texts, false).await {
                Ok(vectors) if vectors.len() == batch.len() => {
                    let rows: Vec<(i64, Vec<f32>)> =
                        batch.iter().map(|(id, _)| *id).zip(vectors).collect();
                    self.index.set_embeddings(&rows).await?;
                    done += batch.len() as u32;
                }
                Ok(_) => {
                    return Ok(Some(
                        "the embedding model answered with the wrong number of vectors".into(),
                    ));
                }
                Err(e) => return Ok(Some(e.to_string())),
            }
        }
    }

    pub async fn graph(&self) -> Result<Graph> {
        Ok(self
            .index
            .meta("graph")
            .await?
            .and_then(|json| serde_json::from_str(&json).ok())
            .unwrap_or_default())
    }

    /// The `k` passages that best match `query`.
    pub async fn search(
        &self,
        embedder: Option<&dyn Embedder>,
        query: &str,
        k: usize,
    ) -> Result<Vec<Passage>> {
        let vectors = self.vectors().await?;
        search::search(&self.index, vectors.as_deref(), embedder, query, k).await
    }

    /// Answers from the library's best passages only, citing them by number.
    pub async fn ask(
        &self,
        model: &dyn Model,
        embedder: Option<&dyn Embedder>,
        question: &str,
        language: &str,
    ) -> Result<Answer> {
        let passages = self.search(embedder, question, ASK_PASSAGES).await?;
        if passages.is_empty() {
            return Ok(Answer::default());
        }
        let mut prompt = String::from("Excerpts from the user's documents:\n");
        for (i, p) in passages.iter().enumerate() {
            let _ = write!(prompt, "\n[{}] {}\n{}\n", i + 1, p.title, p.text);
        }
        let _ = write!(
            prompt,
            "\nQuestion: {question}\n\nAnswer in {language}, using only the excerpts. Right after each statement, \
             put the number of the excerpt that supports it in square brackets, like [2]. If the excerpts do not \
             answer the question, say so plainly. List every number you cited in `citations`."
        );
        let reply = model.json(ASK_SYSTEM, &prompt, &answer_schema()).await?;
        let text = reply["answer"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let listed = reply["citations"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_u64);
        // Numbers in the text come first, in reading order; anything outside the excerpts is dropped.
        let mut cited: Vec<usize> = Vec::new();
        for n in cited_in(&text)
            .into_iter()
            .chain(listed.map(|n| n as usize))
        {
            if (1..=passages.len()).contains(&n) && !cited.contains(&n) {
                cited.push(n);
            }
        }
        Ok(Answer {
            text,
            sources: cited
                .into_iter()
                .map(|n| {
                    let p = &passages[n - 1];
                    Source {
                        n: n as u32,
                        title: p.title.clone(),
                        path: p.path.clone(),
                        excerpt: excerpt(&p.text),
                    }
                })
                .collect(),
        })
    }

    async fn vectors(&self) -> Result<Option<Arc<Vectors>>> {
        if let Some(v) = self.vectors.lock().ok().and_then(|v| v.clone()) {
            return Ok(Some(v));
        }
        let loaded = Arc::new(Vectors::new(self.index.vectors().await?));
        if let Ok(mut cache) = self.vectors.lock() {
            *cache = Some(loaded.clone());
        }
        Ok(Some(loaded))
    }

    fn forget_vectors(&self) {
        if let Ok(mut cache) = self.vectors.lock() {
            *cache = None;
        }
    }
}

const ASK_SYSTEM: &str = "You answer questions about the user's own documents. \
Use only the numbered excerpts you are given and cite them; never add facts from elsewhere.";

fn answer_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "answer": { "type": "string" },
            "citations": { "type": "array", "items": { "type": "integer" } },
        },
        "required": ["answer", "citations"],
        "additionalProperties": false,
    })
}

/// `[2]`, `[1, 3]` and `[1][4]` markers, in order.
fn cited_in(text: &str) -> Vec<usize> {
    let mut out = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find(']') else { break };
        let inside = &rest[..end];
        if inside
            .chars()
            .all(|c| c.is_ascii_digit() || c == ',' || c == ' ')
        {
            out.extend(
                inside
                    .split(',')
                    .filter_map(|n| n.trim().parse::<usize>().ok()),
            );
        }
        rest = &rest[end + 1..];
    }
    out
}

fn excerpt(text: &str) -> String {
    if text.chars().count() <= EXCERPT_CHARS {
        return text.to_owned();
    }
    let cut: String = text.chars().take(EXCERPT_CHARS).collect();
    let cut = cut.rfind(' ').map_or(cut.as_str(), |i| &cut[..i]);
    format!("{cut}…")
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Io(e.to_string()))?
}

/// Supported files under `folders`, skipping hidden folders, dependencies,
/// symlinks and files too big to be documents.
fn scan(folders: &[String]) -> Vec<Found> {
    let mut found = Vec::new();
    let mut stack: Vec<PathBuf> = folders.iter().map(PathBuf::from).collect();
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            // `DirEntry::metadata` does not follow symlinks, so they are skipped.
            let Ok(meta) = entry.metadata() else { continue };
            let path = entry.path();
            if meta.is_dir() {
                stack.push(path);
            } else if meta.is_file()
                && meta.len() <= extract::MAX_FILE_BYTES
                && extract::supported(&path)
            {
                let modified = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                found.push(Found {
                    path: path.to_string_lossy().into_owned(),
                    size: meta.len() as i64,
                    modified,
                });
            }
        }
    }
    // Nested or repeated folders must not index a file twice.
    found.sort_by(|a, b| a.path.cmp(&b.path));
    found.dedup_by(|a, b| a.path == b.path);
    found
}

/// New files, files whose size or date changed, and indexed files no longer found.
fn diff<'a>(
    found: &'a [Found],
    stored: &'a [Stored],
) -> (Vec<&'a Found>, Vec<&'a Found>, Vec<&'a Stored>) {
    let by_path: HashMap<&str, &Stored> = stored.iter().map(|s| (s.path.as_str(), s)).collect();
    let on_disk: HashSet<&str> = found.iter().map(|f| f.path.as_str()).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for f in found {
        match by_path.get(f.path.as_str()) {
            None => added.push(f),
            Some(s) if s.size != f.size || s.modified != f.modified => changed.push(f),
            Some(_) => {}
        }
    }
    let removed = stored
        .iter()
        .filter(|s| !on_disk.contains(s.path.as_str()))
        .collect();
    (added, changed, removed)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;

    /// Vectors from word counts hashed into a few dimensions: texts that share
    /// words end up close, which is all the tests need.
    struct Words {
        calls: Mutex<u32>,
    }

    impl Embedder for Words {
        fn id(&self) -> &str {
            "words"
        }

        fn embed<'a>(
            &'a self,
            texts: &'a [String],
            _: bool,
        ) -> BoxFuture<'a, Result<Vec<Vec<f32>>>> {
            *self.calls.lock().unwrap() += 1;
            let vectors = texts
                .iter()
                .map(|t| {
                    let mut v = vec![0.0; 32];
                    for w in search::terms(t) {
                        let h = w
                            .bytes()
                            .fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
                        v[h as usize % 32] += 1.0;
                    }
                    v
                })
                .collect();
            Box::pin(async move { Ok(vectors) })
        }
    }

    struct Scripted(Mutex<VecDeque<Value>>);

    impl Model for Scripted {
        fn json<'a>(
            &'a self,
            _: &'a str,
            prompt: &'a str,
            _: &'a Value,
        ) -> BoxFuture<'a, Result<Value>> {
            assert!(prompt.contains("[1]"), "excerpts are numbered");
            let next = self.0.lock().unwrap().pop_front().unwrap();
            Box::pin(async move { Ok(next) })
        }
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) {
        std::fs::write(dir.join(name), bytes).unwrap();
    }

    async fn library(name: &str) -> (PathBuf, Library) {
        let dir =
            std::env::temp_dir().join(format!("mcforge-library-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let docs = dir.join("docs");
        std::fs::create_dir_all(docs.join(".oculta")).unwrap();
        write(
            &docs,
            "descuentos.md",
            "# Descuentos\n\nEl descuento máximo autorizado es del 15 %. Ver [[metas]].".as_bytes(),
        );
        write(
            &docs,
            "metas.md",
            "# Metas\n\nLa meta de ventas de la región Norte es de 1.2 millones.".as_bytes(),
        );
        write(
            &docs,
            "vacaciones.docx",
            &extract::tests::docx(&["Vacaciones: doce días hábiles el primer año."]),
        );
        write(
            &docs,
            "viaticos.pdf",
            &extract::tests::pdf(&["Viaticos por dia: 1,800 pesos en hotel y comidas"]),
        );
        write(&docs, "escaneado.pdf", b"%PDF-1.4 not really");
        write(&docs.join(".oculta"), "secreto.md", b"no indexar");
        write(&docs, "foto.png", b"\x89PNG");
        let library = Library::open(&dir.join("library.db")).await.unwrap();
        library
            .set_folders(&[docs.to_string_lossy().into_owned()])
            .await
            .unwrap();
        (dir, library)
    }

    #[tokio::test]
    async fn indexes_incrementally_and_searches_without_vectors() {
        let (dir, library) = library("update").await;
        let docs = dir.join("docs");
        let status = library.status().await.unwrap();
        assert_eq!(
            status.pending.added, 5,
            "hidden folders and other types are skipped"
        );

        let summary = library.update(None, |_| {}).await.unwrap();
        assert_eq!(summary.added, 5);
        assert_eq!(summary.failed.len(), 1, "the fake PDF has no text");
        let status = library.status().await.unwrap();
        assert_eq!((status.files, status.failed), (4, 1));
        assert_eq!(
            status.pending,
            Pending {
                unembedded: status.chunks,
                ..Pending::default()
            }
        );
        assert!(library.contains(&docs.join("metas.md").to_string_lossy()));

        let hits = library
            .search(None, "¿Cuál es el descuento máximo?", 3)
            .await
            .unwrap();
        assert_eq!(hits[0].title, "descuentos");
        let hits = library.search(None, "viaticos hotel", 3).await.unwrap();
        assert_eq!(hits[0].title, "viaticos", "PDF text is searchable");

        std::fs::remove_file(docs.join("metas.md")).unwrap();
        write(
            &docs,
            "descuentos.md",
            "# Descuentos\n\nAhora el máximo es del 20 %.".as_bytes(),
        );
        write(
            &docs,
            "nuevo.txt",
            b"Inventario de fin de mes en el almacen central.",
        );
        let pending = library.status().await.unwrap().pending;
        assert_eq!((pending.added, pending.changed, pending.removed), (1, 1, 1));
        library.update(None, |_| {}).await.unwrap();
        assert!(!library.contains(&docs.join("metas.md").to_string_lossy()));
        let hits = library.search(None, "máximo descuento", 1).await.unwrap();
        assert!(hits[0].text.contains("20 %"), "{}", hits[0].text);
        let pending = library.status().await.unwrap().pending;
        assert_eq!((pending.added, pending.changed, pending.removed), (0, 0, 0));
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn embeds_once_and_answers_with_valid_citations() {
        let (dir, library) = library("ask").await;
        let words = Words {
            calls: Mutex::new(0),
        };
        let summary = library.update(Some(&words), |_| {}).await.unwrap();
        assert_eq!(summary.embed_error, None);
        assert_eq!(library.status().await.unwrap().pending.unembedded, 0);
        let calls = *words.calls.lock().unwrap();
        library.update(Some(&words), |_| {}).await.unwrap();
        assert_eq!(*words.calls.lock().unwrap(), calls, "nothing new to embed");

        let hits = library
            .search(Some(&words), "meta región norte", 2)
            .await
            .unwrap();
        assert_eq!(hits[0].title, "metas");

        let model = Scripted(Mutex::new(VecDeque::from([json!({
            "answer": "La meta del Norte es de 1.2 millones [1]. Dato inventado [9].",
            "citations": [1, 7],
        })])));
        let answer = library
            .ask(
                &model,
                Some(&words),
                "¿Cuál es la meta del Norte?",
                "Spanish",
            )
            .await
            .unwrap();
        assert!(answer.text.contains("[1]"));
        assert_eq!(
            answer.sources.len(),
            1,
            "citations outside the excerpts are dropped"
        );
        assert_eq!(answer.sources[0].title, "metas");

        let graph = library.graph().await.unwrap();
        assert_eq!(graph.nodes.len(), 4);
        assert!(
            graph.edges.iter().any(|e| e.kind == graph::EdgeKind::Link),
            "[[metas]] is drawn"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[tokio::test]
    async fn a_cancelled_update_leaves_a_usable_index() {
        use futures_util::future::{AbortHandle, Abortable};
        let (dir, library) = library("cancel").await;
        let (handle, registration) = AbortHandle::new_pair();
        let work = library.update(None, |p| {
            if matches!(p, Progress::Reading { done: 2, .. }) {
                handle.abort();
            }
        });
        assert!(Abortable::new(work, registration).await.is_err());
        let status = library.status().await.unwrap();
        assert_eq!(
            status.files + status.failed,
            2,
            "files finished before the cancel stay indexed"
        );
        assert_eq!(status.pending.added, 3);
        library.update(None, |_| {}).await.unwrap();
        assert_eq!(library.status().await.unwrap().pending.added, 0);
        std::fs::remove_dir_all(dir).ok();
    }

    /// Indexes a real folder and asks a real local model.
    /// `MCFORGE_TEST_LIBRARY=dir MCFORGE_TEST_OLLAMA_MODEL=qwen3:8b cargo test real_library -- --ignored --nocapture`
    /// (optionally `MCFORGE_TEST_EMBEDDING=qwen3-embedding:0.6b` and `MCFORGE_TEST_QUESTION`).
    #[tokio::test]
    #[ignore = "needs Ollama and a folder of documents"]
    async fn real_library() {
        let (Ok(folder), Ok(model)) = (
            std::env::var("MCFORGE_TEST_LIBRARY"),
            std::env::var("MCFORGE_TEST_OLLAMA_MODEL"),
        ) else {
            panic!("set MCFORGE_TEST_LIBRARY and MCFORGE_TEST_OLLAMA_MODEL");
        };
        let dir = std::env::temp_dir().join(format!("mcforge-real-library-{}", std::process::id()));
        let library = Library::open(&dir.join("library.db")).await.unwrap();
        library.set_folders(&[folder]).await.unwrap();
        let embedder = std::env::var("MCFORGE_TEST_EMBEDDING")
            .ok()
            .map(|model| OllamaEmbedder {
                http: reqwest::Client::new(),
                model,
                query_prefix: String::new(),
            });
        let embedder = embedder.as_ref().map(|e| e as &dyn Embedder);
        let started = std::time::Instant::now();
        let summary = library.update(embedder, |_| {}).await.unwrap();
        eprintln!("{:.1}s {summary:#?}", started.elapsed().as_secs_f64());
        let graph = library.graph().await.unwrap();
        for (i, c) in graph.clusters.iter().enumerate() {
            let files: Vec<&str> = graph
                .nodes
                .iter()
                .filter(|n| n.cluster == i as u32)
                .map(|n| n.title.as_str())
                .collect();
            eprintln!("topic {i} \"{}\": {files:?}", c.label);
        }
        eprintln!("{} edges", graph.edges.len());
        let question = std::env::var("MCFORGE_TEST_QUESTION").unwrap_or_else(|_| {
            "¿Cuál es el descuento máximo que puede autorizar un vendedor?".into()
        });
        let engine = crate::providers::chat::EngineModel::new(
            reqwest::Client::new(),
            crate::setup::Engine::Local { model },
        )
        .unwrap();
        let started = std::time::Instant::now();
        let answer = library
            .ask(&engine, embedder, &question, "Spanish")
            .await
            .unwrap();
        eprintln!("{:.1}s {answer:#?}", started.elapsed().as_secs_f64());
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn citation_markers_are_read_in_order() {
        assert_eq!(cited_in("Uno [2]. Dos [1, 3][4]. Nota [a]."), [2, 1, 3, 4]);
    }
}
