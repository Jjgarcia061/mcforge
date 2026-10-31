//! Local models through the Ollama daemon (http://127.0.0.1:11434).

use std::path::PathBuf;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

const BASE: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum OllamaStatus {
    NotInstalled,
    Stopped,
    Running {
        version: String,
        models: Vec<String>,
    },
}

/// One line of `/api/pull` progress. Byte counts are f64 so they stay plain numbers in TypeScript.
#[derive(Debug, Clone, Deserialize, Serialize, specta::Type)]
pub struct PullProgress {
    pub status: String,
    pub total: Option<f64>,
    pub completed: Option<f64>,
    #[serde(default, skip_serializing)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct Version {
    version: String,
}

#[derive(Deserialize)]
struct Tags {
    models: Vec<Tag>,
}

#[derive(Deserialize)]
struct Tag {
    name: String,
}

pub async fn status(http: &reqwest::Client) -> Result<OllamaStatus> {
    let Ok(res) = http.get(format!("{BASE}/api/version")).send().await else {
        return Ok(if is_installed() {
            OllamaStatus::Stopped
        } else {
            OllamaStatus::NotInstalled
        });
    };
    let version = res.json::<Version>().await?.version;
    let models = http
        .get(format!("{BASE}/api/tags"))
        .send()
        .await?
        .json::<Tags>()
        .await?
        .models
        .into_iter()
        .map(|t| t.name)
        .collect();
    Ok(OllamaStatus::Running { version, models })
}

/// Downloads a model, reporting each progress line Ollama streams back.
pub async fn pull(
    http: &reqwest::Client,
    model: &str,
    mut on_progress: impl FnMut(PullProgress),
) -> Result<()> {
    let res = http
        .post(format!("{BASE}/api/pull"))
        .json(&serde_json::json!({ "model": model, "stream": true }))
        .send()
        .await?
        .error_for_status()?;

    let mut stream = res.bytes_stream();
    let mut buf = Vec::new();
    while let Some(chunk) = stream.next().await {
        buf.extend_from_slice(&chunk?);
        while let Some(end) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=end).collect();
            let Ok(progress) = serde_json::from_slice::<PullProgress>(&line) else {
                continue;
            };
            if let Some(e) = progress.error {
                return Err(Error::Provider(e));
            }
            on_progress(progress);
        }
    }
    Ok(())
}

/// GUI apps on macOS get a minimal PATH, so the usual install locations are checked too.
fn is_installed() -> bool {
    let exe = if cfg!(windows) {
        "ollama.exe"
    } else {
        "ollama"
    };
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.extend(
        [
            "/usr/local/bin",
            "/opt/homebrew/bin",
            "/Applications/Ollama.app/Contents/Resources",
        ]
        .map(PathBuf::from),
    );
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        dirs.push(PathBuf::from(local).join("Programs").join("Ollama"));
    }
    dirs.iter().any(|d| d.join(exe).is_file())
}
