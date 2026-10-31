//! Model providers: key validation, model discovery, structured answers and
//! streaming chat with tool calls.

pub mod chat;
pub mod ollama;
pub mod stream;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub(crate) const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Provider {
    Anthropic,
    Openai,
}

impl Provider {
    /// Keychain account under which this provider's API key is stored.
    pub fn key_account(self) -> &'static str {
        match self {
            Self::Anthropic => "anthropic-api-key",
            Self::Openai => "openai-api-key",
        }
    }

    fn models_request(self, http: &reqwest::Client, key: &str) -> reqwest::RequestBuilder {
        match self {
            Self::Anthropic => http
                .get("https://api.anthropic.com/v1/models?limit=1000")
                .header("x-api-key", key)
                .header("anthropic-version", ANTHROPIC_VERSION),
            Self::Openai => http
                .get("https://api.openai.com/v1/models")
                .bearer_auth(key),
        }
    }
}

// Both providers answer `GET /v1/models` with `{ "data": [{ "id": … }] }`.
#[derive(Deserialize)]
struct ModelList {
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

/// Lists the models a key can use; doubles as key validation.
pub async fn list_models(
    http: &reqwest::Client,
    provider: Provider,
    key: &str,
) -> Result<Vec<String>> {
    let res = provider.models_request(http, key.trim()).send().await?;
    match res.status() {
        s if s == reqwest::StatusCode::UNAUTHORIZED || s == reqwest::StatusCode::FORBIDDEN => {
            Err(Error::Provider("The API key was rejected.".into()))
        }
        s if !s.is_success() => Err(Error::Provider(format!("The provider answered {s}."))),
        _ => {
            let mut ids: Vec<_> = res
                .json::<ModelList>()
                .await?
                .data
                .into_iter()
                .map(|m| m.id)
                .collect();
            ids.sort();
            Ok(ids)
        }
    }
}
