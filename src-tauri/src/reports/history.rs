//! Reports generated on this computer, newest first, so they can be found and
//! reopened after a restart. It is also what `open_report` checks: MCForge only
//! opens files it wrote itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::ReportOutcome;
use crate::atomic::Stored;
use crate::error::Result;

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ReportEntry {
    pub path: String,
    pub title: String,
    /// The data it was made from: the file name, plus the sheet for workbooks.
    pub source: String,
    /// RFC 3339 in local time.
    pub created: String,
    pub sections: u32,
    pub engine: Option<String>,
}

impl ReportEntry {
    pub fn new(outcome: &ReportOutcome, source: String) -> Self {
        Self {
            path: outcome.path.clone(),
            title: outcome.title.clone(),
            source,
            created: chrono::Local::now().to_rfc3339(),
            sections: outcome.sections,
            engine: outcome.engine.clone(),
        }
    }
}

pub struct History(Stored<Vec<ReportEntry>>);

impl History {
    pub fn load(path: PathBuf) -> Self {
        Self(Stored::load(path))
    }

    pub fn add(&self, entry: ReportEntry) -> Result<()> {
        self.0.update(|entries| {
            entries.retain(|e| e.path != entry.path);
            entries.insert(0, entry);
            entries.truncate(MAX_ENTRIES);
        })
    }

    /// Entries whose file still exists.
    pub fn list(&self) -> Vec<ReportEntry> {
        self.0.read(|entries| {
            entries
                .iter()
                .filter(|e| Path::new(&e.path).is_file())
                .cloned()
                .collect()
        })
    }

    pub fn contains(&self, path: &Path) -> bool {
        self.0
            .read(|entries| entries.iter().any(|e| Path::new(&e.path) == path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(dir: &Path, name: &str) -> ReportEntry {
        let path = dir.join(format!("{name}.html"));
        std::fs::write(&path, "<html></html>").unwrap();
        ReportEntry {
            path: path.to_string_lossy().into_owned(),
            title: name.into(),
            source: "ventas.csv".into(),
            created: "2026-10-30T12:00:00-06:00".into(),
            sections: 3,
            engine: None,
        }
    }

    fn titles(entries: &[ReportEntry]) -> Vec<&str> {
        entries.iter().map(|e| e.title.as_str()).collect()
    }

    #[test]
    fn keeps_reports_across_restarts_newest_first() {
        let dir = std::env::temp_dir().join(format!("mcforge-history-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("reports.json");
        let history = History::load(file.clone());
        let (a, b) = (entry(&dir, "a"), entry(&dir, "b"));
        history.add(a.clone()).unwrap();
        history.add(b.clone()).unwrap();
        history.add(a.clone()).unwrap();
        assert_eq!(
            titles(&history.list()),
            ["a", "b"],
            "a regenerated path moves to the top once"
        );

        let reloaded = History::load(file.clone());
        assert_eq!(reloaded.list(), history.list());
        assert!(reloaded.contains(Path::new(&b.path)));
        assert!(!reloaded.contains(&dir.join("other.html")));

        std::fs::remove_file(&a.path).unwrap();
        assert_eq!(
            titles(&reloaded.list()),
            ["b"],
            "deleted files are not listed"
        );
        std::fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn is_capped_and_survives_a_damaged_file() {
        let dir = std::env::temp_dir().join(format!("mcforge-history-cap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("reports.json");
        std::fs::write(&file, "{ not json").unwrap();
        let history = History::load(file.clone());
        assert!(history.list().is_empty());

        let first = entry(&dir, "first");
        history.add(first.clone()).unwrap();
        for i in 0..MAX_ENTRIES {
            history.add(entry(&dir, &i.to_string())).unwrap();
        }
        let list = History::load(file).list();
        assert_eq!(list.len(), MAX_ENTRIES);
        assert!(!list.contains(&first), "the oldest entry is dropped");
        std::fs::remove_dir_all(dir).ok();
    }
}
