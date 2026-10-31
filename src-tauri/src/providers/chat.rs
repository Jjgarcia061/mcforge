//! One-shot structured answers from the engine chosen in the setup, for reports
//! and library answers. `stream.rs` adds streaming and tool calls for the chat.

use std::time::Duration;

use futures_util::future::BoxFuture;
use serde_json::{Value, json};

use super::{ANTHROPIC_VERSION, Provider, ollama};
use crate::error::{Error, Result};
use crate::secrets;
use crate::setup::Engine;

/// Local models can take minutes on modest hardware.
pub(super) const TIMEOUT: Duration = Duration::from_secs(600);
/// Ollama's default context is small enough to silently cut a data summary.
pub(super) const LOCAL_CONTEXT: u32 = 16_384;
pub(super) const MAX_TOKENS: u32 = 8_192;
const ANSWER_TOOL: &str = "answer";

/// Anything that answers a prompt with JSON matching a schema.
pub trait Model: Send + Sync {
    fn json<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
        schema: &'a Value,
    ) -> BoxFuture<'a, Result<Value>>;
}

pub struct EngineModel {
    pub(super) http: reqwest::Client,
    pub(super) engine: Engine,
    pub(super) key: String,
}

impl EngineModel {
    pub fn new(http: reqwest::Client, engine: Engine) -> Result<Self> {
        let key = match &engine {
            Engine::Local { .. } => String::new(),
            Engine::Api { provider, .. } => {
                secrets::get(provider.key_account())?.ok_or_else(|| {
                    Error::Provider("No API key is saved yet. Add one in the setup.".into())
                })?
            }
        };
        Ok(Self { http, engine, key })
    }

    async fn ask(&self, system: &str, prompt: &str, schema: &Value) -> Result<Value> {
        let request = match &self.engine {
            Engine::Local { model } => self.http.post(format!("{}/api/chat", ollama::BASE)).json(&json!({
                "model": model,
                "stream": false,
                "format": schema,
                "messages": [
                    { "role": "system", "content": system },
                    { "role": "user", "content": prompt },
                ],
                "options": { "temperature": 0.2, "num_ctx": LOCAL_CONTEXT },
            })),
            // A forced tool call is how the Messages API guarantees input matching a schema.
            Engine::Api { provider: Provider::Anthropic, model } => self
                .http
                .post("https://api.anthropic.com/v1/messages")
                .header("x-api-key", &self.key)
                .header("anthropic-version", ANTHROPIC_VERSION)
                .json(&json!({
                    "model": model,
                    "max_tokens": MAX_TOKENS,
                    "system": system,
                    "messages": [{ "role": "user", "content": prompt }],
                    "tools": [{ "name": ANSWER_TOOL, "description": "Return the answer.", "input_schema": schema }],
                    "tool_choice": { "type": "tool", "name": ANSWER_TOOL },
                })),
            Engine::Api { provider: Provider::Openai, model } => self
                .http
                .post("https://api.openai.com/v1/chat/completions")
                .bearer_auth(&self.key)
                .json(&json!({
                    "model": model,
                    "messages": [
                        { "role": "system", "content": system },
                        { "role": "user", "content": prompt },
                    ],
                    "response_format": {
                        "type": "json_schema",
                        "json_schema": { "name": ANSWER_TOOL, "strict": true, "schema": schema },
                    },
                })),
        };

        let res = request.timeout(TIMEOUT).send().await.map_err(|e| {
            if e.is_connect() && matches!(self.engine, Engine::Local { .. }) {
                Error::Provider("Ollama is not running. Open it and try again.".into())
            } else {
                e.into()
            }
        })?;
        let status = res.status();
        let text = res.text().await?;
        if !status.is_success() {
            return Err(Error::Provider(failure(status, &text)));
        }
        let body: Value = serde_json::from_str(&text)
            .map_err(|e| Error::Provider(format!("Unexpected answer from the model: {e}")))?;

        match &self.engine {
            Engine::Local { .. } => {
                parse_json_text(body["message"]["content"].as_str().unwrap_or_default())
            }
            Engine::Api {
                provider: Provider::Anthropic,
                ..
            } => {
                if body["stop_reason"] == "max_tokens" {
                    return Err(Error::Provider(
                        "The answer was cut off before it finished.".into(),
                    ));
                }
                body["content"]
                    .as_array()
                    .and_then(|blocks| blocks.iter().find(|b| b["type"] == "tool_use"))
                    .map(|b| b["input"].clone())
                    .ok_or_else(|| Error::Provider("The model did not return an answer.".into()))
            }
            Engine::Api {
                provider: Provider::Openai,
                ..
            } => {
                let message = &body["choices"][0]["message"];
                if let Some(refusal) = message["refusal"].as_str() {
                    return Err(Error::Provider(format!("The model refused: {refusal}")));
                }
                parse_json_text(message["content"].as_str().unwrap_or_default())
            }
        }
    }
}

impl Model for EngineModel {
    fn json<'a>(
        &'a self,
        system: &'a str,
        prompt: &'a str,
        schema: &'a Value,
    ) -> BoxFuture<'a, Result<Value>> {
        Box::pin(self.ask(system, prompt, schema))
    }
}

/// The provider's own error message when it sent one.
pub(super) fn failure(status: reqwest::StatusCode, body: &str) -> String {
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return "The API key was rejected.".into();
    }
    let parsed: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let message = parsed["error"]["message"]
        .as_str()
        .or_else(|| parsed["error"].as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| body.chars().take(300).collect());
    format!("The model answered {status}: {message}")
}

/// JSON from a text answer, tolerating code fences or prose around it.
fn parse_json_text(text: &str) -> Result<Value> {
    let start = text.find('{');
    let end = text.rfind('}');
    let json = match (start, end) {
        (Some(s), Some(e)) if s < e => &text[s..=e],
        _ => text,
    };
    serde_json::from_str(json)
        .map_err(|e| Error::Provider(format!("The model did not answer with valid JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_json_from_chatty_answers() {
        let v = parse_json_text("Sure!\n```json\n{\"a\": {\"b\": 1}}\n```").unwrap();
        assert_eq!(v, json!({ "a": { "b": 1 } }));
        assert!(parse_json_text("no json here").is_err());
    }

    #[test]
    fn surfaces_provider_errors() {
        let msg = failure(
            reqwest::StatusCode::NOT_FOUND,
            r#"{"error":"model 'x' not found, try pulling it first"}"#,
        );
        assert!(msg.contains("not found"), "{msg}");
        let msg = failure(reqwest::StatusCode::UNAUTHORIZED, "");
        assert_eq!(msg, "The API key was rejected.");
    }
}
