//! Who a report is written for. Presets live in the catalog; the user's own
//! focuses and the last one used are kept in a small JSON file.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::atomic::Stored;
use crate::error::{Error, Result};

/// Ids of saved focuses start with this, so they never clash with a preset.
pub const CUSTOM_PREFIX: &str = "custom-";

/// A focus as the UI resolved it: presets arrive with their localized name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
pub struct Focus {
    pub id: String,
    pub name: String,
    pub instructions: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize, specta::Type)]
pub struct SavedFocuses {
    pub custom: Vec<Focus>,
    /// Preset or custom id of the focus used last.
    pub last: Option<String>,
}

pub struct Focuses(Stored<SavedFocuses>);

impl Focuses {
    pub fn load(path: PathBuf) -> Self {
        Self(Stored::load(path))
    }

    pub fn saved(&self) -> SavedFocuses {
        self.0.read(Clone::clone)
    }

    /// Adds a focus (empty id) or replaces the one with the same id; it becomes the last used.
    pub fn save(&self, mut focus: Focus) -> Result<Focus> {
        focus.name = focus.name.trim().to_owned();
        focus.instructions = focus.instructions.trim().to_owned();
        if focus.name.is_empty() || focus.instructions.is_empty() {
            return Err(Error::Report(
                "a focus needs a name and instructions".into(),
            ));
        }
        if focus.id.is_empty() {
            focus.id = format!("{CUSTOM_PREFIX}{}", chrono::Utc::now().timestamp_millis());
        } else if !focus.id.starts_with(CUSTOM_PREFIX) {
            return Err(Error::Report("presets cannot be changed".into()));
        }
        self.0.update(|saved| {
            match saved.custom.iter_mut().find(|f| f.id == focus.id) {
                Some(existing) => *existing = focus.clone(),
                None => saved.custom.push(focus.clone()),
            }
            saved.last = Some(focus.id.clone());
        })?;
        Ok(focus)
    }

    pub fn delete(&self, id: &str) -> Result<()> {
        self.0.update(|saved| {
            saved.custom.retain(|f| f.id != id);
            if saved.last.as_deref() == Some(id) {
                saved.last = None;
            }
        })
    }

    pub fn remember(&self, id: &str) -> Result<()> {
        if self.0.read(|saved| saved.last.as_deref() == Some(id)) {
            return Ok(());
        }
        self.0.update(|saved| saved.last = Some(id.to_owned()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn focus(id: &str, name: &str) -> Focus {
        Focus {
            id: id.into(),
            name: name.into(),
            instructions: "Write for the board.".into(),
        }
    }

    #[test]
    fn saves_edits_and_deletes_custom_focuses() {
        let dir = std::env::temp_dir().join(format!("mcforge-focus-{}", std::process::id()));
        let file = dir.join("focus.json");
        let focuses = Focuses::load(file.clone());

        let board = focuses.save(focus("", "  Consejo ")).unwrap();
        assert!(board.id.starts_with(CUSTOM_PREFIX));
        assert_eq!(board.name, "Consejo");
        focuses.save(focus(&board.id, "Consejo directivo")).unwrap();
        assert!(
            focuses.save(focus("finance", "Mine")).is_err(),
            "presets stay as shipped"
        );
        assert!(focuses.save(focus("", " ")).is_err());

        let reloaded = Focuses::load(file).saved();
        assert_eq!(reloaded.custom.len(), 1);
        assert_eq!(reloaded.custom[0].name, "Consejo directivo");
        assert_eq!(reloaded.last.as_deref(), Some(board.id.as_str()));

        focuses.remember("finance").unwrap();
        focuses.delete(&board.id).unwrap();
        let saved = focuses.saved();
        assert!(saved.custom.is_empty());
        assert_eq!(saved.last.as_deref(), Some("finance"));
        std::fs::remove_dir_all(dir).ok();
    }
}
