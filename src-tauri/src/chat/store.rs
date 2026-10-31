//! Conversations on disk: one JSON file each in the app's data folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::tools::Attachment;
use super::{Message, Role};
use crate::atomic;
use crate::error::{Error, Result};

const TITLE_CHARS: usize = 60;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Chat {
    pub id: String,
    pub title: String,
    /// RFC 3339.
    pub updated: String,
    pub messages: Vec<Message>,
    /// Files the user attached; the only ones the report tool may read.
    #[serde(default)]
    pub attachments: Vec<Attachment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
pub struct ChatSummary {
    pub id: String,
    pub title: String,
    pub updated: String,
}

impl Chat {
    /// A new conversation, titled after its first question.
    pub fn new(question: &str) -> Self {
        let line = question.lines().next().unwrap_or_default().trim();
        let mut title: String = line.chars().take(TITLE_CHARS).collect();
        if line.chars().count() > TITLE_CHARS {
            title.push('…');
        }
        Self {
            id: format!("chat-{}", chrono::Utc::now().timestamp_millis()),
            title,
            updated: chrono::Local::now().to_rfc3339(),
            messages: Vec::new(),
            attachments: Vec::new(),
        }
    }

    pub fn ask(&mut self, question: &str) {
        self.messages.push(Message::text(Role::User, question));
    }
}

pub struct Chats {
    dir: PathBuf,
}

impl Chats {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Ids come back from the UI, so they must name a file in this folder and nothing else.
    fn path(&self, id: &str) -> Result<PathBuf> {
        let valid = !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        if !valid {
            return Err(Error::Io(format!("invalid conversation id `{id}`")));
        }
        Ok(self.dir.join(format!("{id}.json")))
    }

    /// Newest first; unreadable files are skipped.
    pub fn list(&self) -> Vec<ChatSummary> {
        let mut chats: Vec<ChatSummary> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
            .filter_map(|e| serde_json::from_slice::<Chat>(&std::fs::read(e.path()).ok()?).ok())
            .map(|c| ChatSummary {
                id: c.id,
                title: c.title,
                updated: c.updated,
            })
            .collect();
        chats.sort_by(|a, b| b.updated.cmp(&a.updated));
        chats
    }

    pub fn load(&self, id: &str) -> Result<Chat> {
        let json = std::fs::read(self.path(id)?)?;
        serde_json::from_slice(&json).map_err(|e| Error::Io(format!("conversation `{id}`: {e}")))
    }

    pub fn save(&self, chat: &Chat) -> Result<()> {
        let json = serde_json::to_vec(chat).map_err(|e| Error::Io(e.to_string()))?;
        atomic::write(&self.path(&chat.id)?, &json)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        match std::fs::remove_file(self.path(id)?) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(e.into()),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn saves_lists_and_refuses_odd_ids() {
        let dir = std::env::temp_dir().join(format!("mcforge-chats-{}", std::process::id()));
        let chats = Chats::new(dir.clone());
        let mut chat = Chat::new(
            "¿Cuál es la meta de ventas del Norte para 2026 según los documentos del área comercial?",
        );
        assert!(chat.title.ends_with('…') && chat.title.chars().count() == TITLE_CHARS + 1);
        chat.ask("¿Y la del Sur?");
        chats.save(&chat).unwrap();
        assert_eq!(chats.load(&chat.id).unwrap(), chat);
        assert_eq!(chats.list().len(), 1);
        assert!(chats.load("../secretos").is_err());
        chats.delete(&chat.id).unwrap();
        assert!(chats.list().is_empty());
        chats.delete(&chat.id).unwrap();
        std::fs::remove_dir_all(dir).ok();
    }
}
