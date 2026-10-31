//! Streaming chat with tool calling. Each provider speaks its own wire format
//! (Anthropic and OpenAI over server-sent events, Ollama as JSON lines); a
//! small decoder per provider turns it into the same `StreamEvent`s.

use std::collections::BTreeMap;

use futures_util::stream::{self, BoxStream, StreamExt};
use serde_json::{Value, json};

use super::chat::{EngineModel, LOCAL_CONTEXT, MAX_TOKENS, TIMEOUT, failure};
use super::{ANTHROPIC_VERSION, Provider, ollama};
use crate::chat::{Message, Part, Role};
use crate::error::{Error, Result};
use crate::setup::Engine;

/// A tool the model may call, with a JSON schema for its input.
#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub schema: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    Text(String),
    ToolCall {
        id: String,
        name: String,
        input: Value,
    },
}

/// Anything that streams an answer to a conversation, possibly calling tools.
pub trait ChatModel: Send + Sync {
    fn stream<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxStream<'a, Result<StreamEvent>>;
}

impl ChatModel for EngineModel {
    fn stream<'a>(
        &'a self,
        system: &'a str,
        messages: &'a [Message],
        tools: &'a [ToolSpec],
    ) -> BoxStream<'a, Result<StreamEvent>> {
        let start = async move {
            let (request, decoder): (_, Box<dyn Decoder>) = match &self.engine {
                Engine::Local { model } => (
                    self.http
                        .post(format!("{}/api/chat", ollama::BASE))
                        .json(&ollama_body(model, system, messages, tools)),
                    Box::new(OllamaDecoder::default()),
                ),
                Engine::Api {
                    provider: Provider::Anthropic,
                    model,
                } => (
                    self.http
                        .post("https://api.anthropic.com/v1/messages")
                        .header("x-api-key", &self.key)
                        .header("anthropic-version", ANTHROPIC_VERSION)
                        .json(&anthropic_body(model, system, messages, tools)),
                    Box::new(AnthropicDecoder::default()),
                ),
                Engine::Api {
                    provider: Provider::Openai,
                    model,
                } => (
                    self.http
                        .post("https://api.openai.com/v1/chat/completions")
                        .bearer_auth(&self.key)
                        .json(&openai_body(model, system, messages, tools)),
                    Box::new(OpenAiDecoder::default()),
                ),
            };
            let res = request.timeout(TIMEOUT).send().await.map_err(|e| {
                if e.is_connect() && matches!(self.engine, Engine::Local { .. }) {
                    Error::Provider("Ollama is not running. Open it and try again.".into())
                } else {
                    e.into()
                }
            })?;
            let status = res.status();
            if !status.is_success() {
                return Err(Error::Provider(failure(status, &res.text().await?)));
            }
            Ok(decode(lines(res), decoder))
        };
        stream::once(start)
            .flat_map(|started| match started {
                Ok(events) => events,
                Err(e) => stream::once(async { Err(e) }).boxed(),
            })
            .boxed()
    }
}

/// The body of a response, line by line.
fn lines(res: reqwest::Response) -> BoxStream<'static, Result<String>> {
    stream::unfold(
        (res.bytes_stream(), Vec::<u8>::new(), false),
        |(mut bytes, mut buf, mut ended)| async move {
            loop {
                if let Some(end) = buf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=end).collect();
                    let text = String::from_utf8_lossy(&line).trim_end().to_owned();
                    return Some((Ok(text), (bytes, buf, ended)));
                }
                if ended {
                    if buf.is_empty() {
                        return None;
                    }
                    let text = String::from_utf8_lossy(&std::mem::take(&mut buf))
                        .trim_end()
                        .to_owned();
                    return Some((Ok(text), (bytes, buf, ended)));
                }
                match bytes.next().await {
                    Some(Ok(chunk)) => buf.extend_from_slice(&chunk),
                    Some(Err(e)) => return Some((Err(e.into()), (bytes, Vec::new(), true))),
                    None => ended = true,
                }
            }
        },
    )
    .boxed()
}

/// Turns one provider's lines into events; `finish` flushes what is still pending.
trait Decoder: Send {
    fn line(&mut self, line: &str) -> Result<Vec<StreamEvent>>;
    fn finish(&mut self) -> Result<Vec<StreamEvent>> {
        Ok(Vec::new())
    }
}

fn decode(
    lines: BoxStream<'static, Result<String>>,
    decoder: Box<dyn Decoder>,
) -> BoxStream<'static, Result<StreamEvent>> {
    stream::unfold(
        (lines, decoder, false),
        |(mut lines, mut decoder, done)| async move {
            if done {
                return None;
            }
            let batch = match lines.next().await {
                Some(Ok(line)) => decoder.line(&line),
                Some(Err(e)) => Err(e),
                None => {
                    let rest = decoder.finish();
                    return Some((rest, (lines, decoder, true)));
                }
            };
            Some((batch, (lines, decoder, false)))
        },
    )
    .flat_map(|batch| match batch {
        Ok(events) => stream::iter(events.into_iter().map(Ok)).boxed(),
        Err(e) => stream::once(async { Err(e) }).boxed(),
    })
    .boxed()
}

/// The JSON payload of a server-sent `data:` line, if it has one.
fn sse_data(line: &str) -> Option<&str> {
    line.strip_prefix("data:")
        .map(str::trim)
        .filter(|d| !d.is_empty())
}

fn parse_input(json: &str) -> Value {
    if json.trim().is_empty() {
        return json!({});
    }
    serde_json::from_str(json).unwrap_or_else(|_| json!({ "_unparsed": json }))
}

// --- Anthropic Messages API -------------------------------------------------

fn anthropic_body(model: &str, system: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let messages: Vec<Value> = messages
        .iter()
        .filter_map(|m| {
            let content: Vec<Value> = m
                .parts
                .iter()
                .filter_map(|p| match p {
                    // The API rejects empty text blocks.
                    Part::Text { text } if text.trim().is_empty() => None,
                    Part::Text { text } => Some(json!({ "type": "text", "text": text })),
                    Part::ToolCall { id, name, input } => {
                        Some(json!({ "type": "tool_use", "id": id, "name": name, "input": input }))
                    }
                    Part::ToolResult {
                        id,
                        output,
                        is_error,
                        ..
                    } => Some(json!({
                        "type": "tool_result", "tool_use_id": id, "content": output, "is_error": is_error,
                    })),
                })
                .collect();
            (!content.is_empty()).then(|| json!({ "role": role(m.role), "content": content }))
        })
        .collect();
    let mut body = json!({
        "model": model,
        "max_tokens": MAX_TOKENS,
        "system": system,
        "messages": messages,
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = tools
            .iter()
            .map(|t| json!({ "name": t.name, "description": t.description, "input_schema": t.schema }))
            .collect();
    }
    body
}

#[derive(Default)]
struct AnthropicDecoder {
    /// Tool calls being streamed, by content block index: id, name, JSON so far.
    tools: BTreeMap<u64, (String, String, String)>,
}

impl Decoder for AnthropicDecoder {
    fn line(&mut self, line: &str) -> Result<Vec<StreamEvent>> {
        let Some(data) = sse_data(line) else {
            return Ok(Vec::new());
        };
        let event: Value = serde_json::from_str(data)
            .map_err(|e| Error::Provider(format!("Unexpected stream from Anthropic: {e}")))?;
        let index = event["index"].as_u64().unwrap_or(0);
        Ok(match event["type"].as_str().unwrap_or_default() {
            "content_block_start" => {
                let block = &event["content_block"];
                match block["type"].as_str() {
                    Some("tool_use") => {
                        let id = block["id"].as_str().unwrap_or_default().to_owned();
                        let name = block["name"].as_str().unwrap_or_default().to_owned();
                        self.tools.insert(index, (id, name, String::new()));
                        Vec::new()
                    }
                    Some("text") => text_event(block["text"].as_str()),
                    _ => Vec::new(),
                }
            }
            "content_block_delta" => {
                let delta = &event["delta"];
                match delta["type"].as_str() {
                    Some("text_delta") => text_event(delta["text"].as_str()),
                    Some("input_json_delta") => {
                        if let Some(tool) = self.tools.get_mut(&index) {
                            tool.2
                                .push_str(delta["partial_json"].as_str().unwrap_or_default());
                        }
                        Vec::new()
                    }
                    _ => Vec::new(),
                }
            }
            "content_block_stop" => self
                .tools
                .remove(&index)
                .map(|(id, name, json)| StreamEvent::ToolCall {
                    id,
                    name,
                    input: parse_input(&json),
                })
                .into_iter()
                .collect(),
            "message_delta" if event["delta"]["stop_reason"] == "max_tokens" => {
                return Err(Error::Provider(
                    "The answer was cut off before it finished.".into(),
                ));
            }
            "error" => {
                return Err(Error::Provider(
                    event["error"]["message"]
                        .as_str()
                        .unwrap_or("Anthropic reported an error.")
                        .to_owned(),
                ));
            }
            _ => Vec::new(),
        })
    }
}

fn text_event(text: Option<&str>) -> Vec<StreamEvent> {
    text.filter(|t| !t.is_empty())
        .map(|t| StreamEvent::Text(t.to_owned()))
        .into_iter()
        .collect()
}

fn role(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

// --- OpenAI chat completions ------------------------------------------------

/// OpenAI and Ollama share this message shape: tool results are messages of
/// their own, and tool calls hang off the assistant message.
fn function_messages(system: &str, messages: &[Message], ollama: bool) -> Vec<Value> {
    let mut out = vec![json!({ "role": "system", "content": system })];
    let mut names: BTreeMap<&str, &str> = BTreeMap::new();
    for m in messages {
        let text: String = m
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::Text { text } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let calls: Vec<Value> = m
            .parts
            .iter()
            .filter_map(|p| match p {
                Part::ToolCall { id, name, input } => {
                    names.insert(id, name);
                    Some(if ollama {
                        json!({ "function": { "name": name, "arguments": input } })
                    } else {
                        json!({
                            "id": id, "type": "function",
                            "function": { "name": name, "arguments": input.to_string() },
                        })
                    })
                }
                _ => None,
            })
            .collect();
        for p in &m.parts {
            if let Part::ToolResult { id, output, .. } = p {
                out.push(if ollama {
                    json!({ "role": "tool", "content": output, "tool_name": names.get(id.as_str()).copied().unwrap_or_default() })
                } else {
                    json!({ "role": "tool", "tool_call_id": id, "content": output })
                });
            }
        }
        if !text.is_empty() || !calls.is_empty() {
            let mut message = json!({ "role": role(m.role), "content": text });
            if !calls.is_empty() {
                message["tool_calls"] = Value::Array(calls);
            }
            out.push(message);
        }
    }
    out
}

fn function_tools(tools: &[ToolSpec]) -> Value {
    tools
        .iter()
        .map(|t| {
            json!({
                "type": "function",
                "function": { "name": t.name, "description": t.description, "parameters": t.schema },
            })
        })
        .collect()
}

fn openai_body(model: &str, system: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut body = json!({
        "model": model,
        "messages": function_messages(system, messages, false),
        "stream": true,
    });
    if !tools.is_empty() {
        body["tools"] = function_tools(tools);
    }
    body
}

#[derive(Default)]
struct OpenAiDecoder {
    /// Tool calls being streamed, by index: id, name, arguments so far.
    tools: BTreeMap<u64, (String, String, String)>,
}

impl OpenAiDecoder {
    fn flush(&mut self) -> Vec<StreamEvent> {
        std::mem::take(&mut self.tools)
            .into_values()
            .map(|(id, name, json)| StreamEvent::ToolCall {
                id,
                name,
                input: parse_input(&json),
            })
            .collect()
    }
}

impl Decoder for OpenAiDecoder {
    fn line(&mut self, line: &str) -> Result<Vec<StreamEvent>> {
        let Some(data) = sse_data(line) else {
            return Ok(Vec::new());
        };
        if data == "[DONE]" {
            return Ok(self.flush());
        }
        let chunk: Value = serde_json::from_str(data)
            .map_err(|e| Error::Provider(format!("Unexpected stream from OpenAI: {e}")))?;
        if let Some(message) = chunk["error"]["message"].as_str() {
            return Err(Error::Provider(message.to_owned()));
        }
        let choice = &chunk["choices"][0];
        let delta = &choice["delta"];
        let mut events = text_event(delta["content"].as_str());
        for call in delta["tool_calls"].as_array().into_iter().flatten() {
            let entry = self
                .tools
                .entry(call["index"].as_u64().unwrap_or(0))
                .or_default();
            if let Some(id) = call["id"].as_str() {
                entry.0 = id.to_owned();
            }
            if let Some(name) = call["function"]["name"].as_str() {
                entry.1.push_str(name);
            }
            if let Some(args) = call["function"]["arguments"].as_str() {
                entry.2.push_str(args);
            }
        }
        if let Some(refusal) = delta["refusal"].as_str().filter(|r| !r.is_empty()) {
            return Err(Error::Provider(format!("The model refused: {refusal}")));
        }
        if choice["finish_reason"].is_string() {
            events.extend(self.flush());
        }
        Ok(events)
    }

    fn finish(&mut self) -> Result<Vec<StreamEvent>> {
        Ok(self.flush())
    }
}

// --- Ollama -----------------------------------------------------------------

fn ollama_body(model: &str, system: &str, messages: &[Message], tools: &[ToolSpec]) -> Value {
    let mut body = json!({
        "model": model,
        "messages": function_messages(system, messages, true),
        "stream": true,
        "options": { "temperature": 0.3, "num_ctx": LOCAL_CONTEXT },
    });
    if !tools.is_empty() {
        body["tools"] = function_tools(tools);
    }
    body
}

#[derive(Default)]
struct OllamaDecoder {
    /// Ollama does not name tool calls, so they are numbered.
    calls: u32,
}

impl Decoder for OllamaDecoder {
    fn line(&mut self, line: &str) -> Result<Vec<StreamEvent>> {
        if line.trim().is_empty() {
            return Ok(Vec::new());
        }
        let chunk: Value = serde_json::from_str(line)
            .map_err(|e| Error::Provider(format!("Unexpected stream from Ollama: {e}")))?;
        if let Some(error) = chunk["error"].as_str() {
            return Err(Error::Provider(error.to_owned()));
        }
        let message = &chunk["message"];
        let mut events = text_event(message["content"].as_str());
        for call in message["tool_calls"].as_array().into_iter().flatten() {
            self.calls += 1;
            let function = &call["function"];
            let input = match &function["arguments"] {
                Value::String(s) => parse_input(s),
                Value::Null => json!({}),
                other => other.clone(),
            };
            events.push(StreamEvent::ToolCall {
                id: call["id"]
                    .as_str()
                    .map_or_else(|| format!("call_{}", self.calls), str::to_owned),
                name: function["name"].as_str().unwrap_or_default().to_owned(),
                input,
            });
        }
        Ok(events)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(decoder: &mut dyn Decoder, body: &str) -> Result<Vec<StreamEvent>> {
        let mut events = Vec::new();
        for line in body.lines() {
            events.extend(decoder.line(line)?);
        }
        events.extend(decoder.finish()?);
        Ok(events)
    }

    fn text(t: &str) -> StreamEvent {
        StreamEvent::Text(t.into())
    }

    #[test]
    fn anthropic_text_and_a_tool_call_split_in_pieces() {
        let body = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","content":[]}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Voy a buscar"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"library_search","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\": \"meta"}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":" norte\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"}}

event: message_stop
data: {"type":"message_stop"}
"#;
        let events = run(&mut AnthropicDecoder::default(), body).unwrap();
        assert_eq!(
            events,
            [
                text("Voy a buscar"),
                StreamEvent::ToolCall {
                    id: "toolu_1".into(),
                    name: "library_search".into(),
                    input: json!({ "query": "meta norte" }),
                },
            ]
        );
        let error =
            r#"data: {"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
        assert!(run(&mut AnthropicDecoder::default(), error).is_err());
    }

    #[test]
    fn openai_tool_call_arguments_arrive_in_fragments() {
        let body = r#"data: {"choices":[{"index":0,"delta":{"role":"assistant","content":"Hola"},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"id":"call_a","type":"function","function":{"name":"database_query","arguments":""}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"sql\":\"SELECT"}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{"tool_calls":[{"index":0,"function":{"arguments":" 1\"}"}}]},"finish_reason":null}]}

data: {"choices":[{"index":0,"delta":{},"finish_reason":"tool_calls"}]}

data: [DONE]
"#;
        let events = run(&mut OpenAiDecoder::default(), body).unwrap();
        assert_eq!(
            events,
            [
                text("Hola"),
                StreamEvent::ToolCall {
                    id: "call_a".into(),
                    name: "database_query".into(),
                    input: json!({ "sql": "SELECT 1" }),
                },
            ]
        );
    }

    #[test]
    fn ollama_lines_and_errors() {
        let body = r#"{"model":"qwen2.5","message":{"role":"assistant","content":"La meta"},"done":false}
{"model":"qwen2.5","message":{"role":"assistant","content":" es 1.2"},"done":false}
{"model":"qwen2.5","message":{"role":"assistant","content":"","tool_calls":[{"function":{"name":"library_search","arguments":{"query":"metas"}}}]},"done":false}
{"model":"qwen2.5","message":{"role":"assistant","content":""},"done":true,"done_reason":"stop"}
"#;
        let events = run(&mut OllamaDecoder::default(), body).unwrap();
        assert_eq!(
            events,
            [
                text("La meta"),
                text(" es 1.2"),
                StreamEvent::ToolCall {
                    id: "call_1".into(),
                    name: "library_search".into(),
                    input: json!({ "query": "metas" }),
                },
            ]
        );
        let error = r#"{"error":"registry.ollama.ai/library/x does not support tools"}"#;
        let e = run(&mut OllamaDecoder::default(), error).unwrap_err();
        assert!(e.to_string().contains("does not support tools"));
    }

    #[test]
    fn conversations_map_to_each_wire_format() {
        let messages = vec![
            Message::text(Role::User, "¿Meta del Norte?"),
            Message {
                role: Role::Assistant,
                parts: vec![
                    Part::Text {
                        text: String::new(),
                    },
                    Part::ToolCall {
                        id: "t1".into(),
                        name: "library_search".into(),
                        input: json!({ "query": "meta norte" }),
                    },
                ],
            },
            Message {
                role: Role::User,
                parts: vec![Part::ToolResult {
                    id: "t1".into(),
                    output: "[1] metas: 1.2 millones".into(),
                    is_error: false,
                    view: None,
                }],
            },
        ];
        let tools = [ToolSpec {
            name: "library_search".into(),
            description: "Search".into(),
            schema: json!({ "type": "object" }),
        }];

        let a = anthropic_body("claude", "sys", &messages, &tools);
        assert_eq!(
            a["messages"][1]["content"].as_array().unwrap().len(),
            1,
            "empty text dropped"
        );
        assert_eq!(a["messages"][2]["content"][0]["tool_use_id"], "t1");
        assert_eq!(a["tools"][0]["input_schema"]["type"], "object");

        let o = openai_body("gpt", "sys", &messages, &tools);
        assert_eq!(o["messages"][0]["role"], "system");
        assert_eq!(
            o["messages"][2]["tool_calls"][0]["function"]["arguments"],
            r#"{"query":"meta norte"}"#
        );
        assert_eq!(o["messages"][3]["role"], "tool");
        assert_eq!(o["messages"][3]["tool_call_id"], "t1");

        let l = ollama_body("qwen", "sys", &messages, &tools);
        assert_eq!(
            l["messages"][2]["tool_calls"][0]["function"]["arguments"]["query"],
            "meta norte"
        );
        assert_eq!(l["messages"][3]["tool_name"], "library_search");
    }
}
