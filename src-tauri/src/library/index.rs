//! The library's SQLite file: indexed folders, files, passages with their
//! vectors, a full-text index over the passages and a few settings. Only this
//! module writes SQL.

use std::path::Path;

use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{QueryBuilder, Row};

use crate::error::Result;

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS folders (path TEXT PRIMARY KEY);
CREATE TABLE IF NOT EXISTS files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE,
    title TEXT NOT NULL,
    kind TEXT NOT NULL,
    size INTEGER NOT NULL,
    modified INTEGER NOT NULL,
    hash TEXT NOT NULL,
    links TEXT NOT NULL DEFAULT '[]',
    error TEXT
);
CREATE TABLE IF NOT EXISTS chunks (
    id INTEGER PRIMARY KEY,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    ord INTEGER NOT NULL,
    text TEXT NOT NULL,
    embedding BLOB
);
CREATE INDEX IF NOT EXISTS chunks_file ON chunks(file_id);
CREATE VIRTUAL TABLE IF NOT EXISTS chunks_fts USING fts5(
    text, content='chunks', content_rowid='id', tokenize='unicode61 remove_diacritics 2'
);
CREATE TRIGGER IF NOT EXISTS chunks_ai AFTER INSERT ON chunks BEGIN
    INSERT INTO chunks_fts(rowid, text) VALUES (new.id, new.text);
END;
CREATE TRIGGER IF NOT EXISTS chunks_ad AFTER DELETE ON chunks BEGIN
    INSERT INTO chunks_fts(chunks_fts, rowid, text) VALUES ('delete', old.id, old.text);
END;
";

/// A file as last indexed.
pub struct Stored {
    pub id: i64,
    pub path: String,
    pub size: i64,
    pub modified: i64,
    pub hash: String,
}

/// What a file contributes to the index.
pub struct Entry<'a> {
    pub path: &'a str,
    pub title: &'a str,
    pub kind: &'a str,
    pub size: i64,
    pub modified: i64,
    pub hash: &'a str,
    pub links: &'a [String],
    /// Why the file has no passages, if it has none.
    pub error: Option<&'a str>,
    pub chunks: &'a [String],
}

/// A passage with the file it comes from.
pub struct Passage {
    pub chunk: i64,
    pub file: i64,
    pub path: String,
    pub title: String,
    pub text: String,
}

/// A file for the graph: its passages' text and their vectors, if any.
pub struct GraphFile {
    pub id: i64,
    pub path: String,
    pub title: String,
    pub kind: String,
    pub links: Vec<String>,
    pub text: String,
    pub vectors: Vec<Vec<f32>>,
}

pub struct Counts {
    pub files: u32,
    pub chunks: u32,
    pub failed: u32,
    pub unembedded: u32,
}

pub struct Index {
    pool: SqlitePool,
}

impl Index {
    pub async fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let opts = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await?;
        sqlx::raw_sql(SCHEMA).execute(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn meta(&self, key: &str) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT value FROM meta WHERE key = ?")
            .bind(key)
            .fetch_optional(&self.pool)
            .await?)
    }

    pub async fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO meta (key, value) VALUES (?, ?) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(value)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn folders(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT path FROM folders ORDER BY path")
            .fetch_all(&self.pool)
            .await?)
    }

    pub async fn set_folders(&self, folders: &[String]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM folders").execute(&mut *tx).await?;
        for f in folders {
            sqlx::query("INSERT OR IGNORE INTO folders (path) VALUES (?)")
                .bind(f)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn files(&self) -> Result<Vec<Stored>> {
        let rows = sqlx::query("SELECT id, path, size, modified, hash FROM files")
            .fetch_all(&self.pool)
            .await?;
        Ok(rows
            .into_iter()
            .map(|r| Stored {
                id: r.get(0),
                path: r.get(1),
                size: r.get(2),
                modified: r.get(3),
                hash: r.get(4),
            })
            .collect())
    }

    pub async fn counts(&self) -> Result<Counts> {
        let row = sqlx::query(
            "SELECT (SELECT COUNT(*) FROM files WHERE error IS NULL), (SELECT COUNT(*) FROM chunks), \
             (SELECT COUNT(*) FROM files WHERE error IS NOT NULL), (SELECT COUNT(*) FROM chunks WHERE embedding IS NULL)",
        )
        .fetch_one(&self.pool)
        .await?;
        let n = |i: usize| u32::try_from(row.get::<i64, _>(i)).unwrap_or(0);
        Ok(Counts {
            files: n(0),
            chunks: n(1),
            failed: n(2),
            unembedded: n(3),
        })
    }

    pub async fn remove(&self, ids: &[i64]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for id in ids {
            sqlx::query("DELETE FROM chunks WHERE file_id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
            sqlx::query("DELETE FROM files WHERE id = ?")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Replaces everything indexed for `entry.path` in one transaction, so a
    /// cancelled update never leaves a file half indexed.
    pub async fn put(&self, entry: &Entry<'_>) -> Result<()> {
        let links = serde_json::to_string(entry.links).unwrap_or_else(|_| "[]".into());
        let mut tx = self.pool.begin().await?;
        sqlx::query("DELETE FROM chunks WHERE file_id = (SELECT id FROM files WHERE path = ?)")
            .bind(entry.path)
            .execute(&mut *tx)
            .await?;
        let id: i64 = sqlx::query_scalar(
            "INSERT INTO files (path, title, kind, size, modified, hash, links, error) VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(path) DO UPDATE SET title = excluded.title, kind = excluded.kind, size = excluded.size, \
             modified = excluded.modified, hash = excluded.hash, links = excluded.links, error = excluded.error \
             RETURNING id",
        )
        .bind(entry.path)
        .bind(entry.title)
        .bind(entry.kind)
        .bind(entry.size)
        .bind(entry.modified)
        .bind(entry.hash)
        .bind(links)
        .bind(entry.error)
        .fetch_one(&mut *tx)
        .await?;
        for (ord, text) in entry.chunks.iter().enumerate() {
            sqlx::query("INSERT INTO chunks (file_id, ord, text) VALUES (?, ?, ?)")
                .bind(id)
                .bind(i64::try_from(ord).unwrap_or(i64::MAX))
                .bind(text)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// A file that changed on disk but not in content keeps its passages.
    pub async fn touch(&self, id: i64, size: i64, modified: i64) -> Result<()> {
        sqlx::query("UPDATE files SET size = ?, modified = ? WHERE id = ?")
            .bind(size)
            .bind(modified)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn unembedded(&self, limit: u32) -> Result<Vec<(i64, String)>> {
        let rows =
            sqlx::query("SELECT id, text FROM chunks WHERE embedding IS NULL ORDER BY id LIMIT ?")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(|r| (r.get(0), r.get(1))).collect())
    }

    pub async fn set_embeddings(&self, vectors: &[(i64, Vec<f32>)]) -> Result<()> {
        let mut tx = self.pool.begin().await?;
        for (id, v) in vectors {
            sqlx::query("UPDATE chunks SET embedding = ? WHERE id = ?")
                .bind(to_blob(v))
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    pub async fn clear_embeddings(&self) -> Result<()> {
        sqlx::query("UPDATE chunks SET embedding = NULL")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Every passage vector, with its passage and file ids.
    pub async fn vectors(&self) -> Result<Vec<(i64, i64, Vec<f32>)>> {
        let rows =
            sqlx::query("SELECT id, file_id, embedding FROM chunks WHERE embedding IS NOT NULL")
                .fetch_all(&self.pool)
                .await?;
        Ok(rows
            .into_iter()
            .map(|r| (r.get(0), r.get(1), from_blob(r.get::<&[u8], _>(2))))
            .collect())
    }

    /// Passages matching any of `terms`, best first (BM25).
    pub async fn keyword(&self, terms: &[String], limit: u32) -> Result<Vec<i64>> {
        if terms.is_empty() {
            return Ok(Vec::new());
        }
        // Quoted terms are plain words to FTS5, never query syntax.
        let query = terms
            .iter()
            .map(|t| format!("\"{}\"", t.replace('"', "")))
            .collect::<Vec<_>>()
            .join(" OR ");
        Ok(sqlx::query_scalar(
            "SELECT rowid FROM chunks_fts WHERE chunks_fts MATCH ? ORDER BY rank LIMIT ?",
        )
        .bind(query)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?)
    }

    /// The passages with these ids, in the same order.
    pub async fn passages(&self, ids: &[i64]) -> Result<Vec<Passage>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut q = QueryBuilder::new(
            "SELECT c.id, c.file_id, f.path, f.title, c.text FROM chunks c JOIN files f ON f.id = c.file_id WHERE c.id IN (",
        );
        let mut list = q.separated(", ");
        for id in ids {
            list.push_bind(id);
        }
        q.push(")");
        let mut found: Vec<Passage> = q
            .build()
            .fetch_all(&self.pool)
            .await?
            .into_iter()
            .map(|r| Passage {
                chunk: r.get(0),
                file: r.get(1),
                path: r.get(2),
                title: r.get(3),
                text: r.get(4),
            })
            .collect();
        found.sort_by_key(|p| ids.iter().position(|&id| id == p.chunk));
        Ok(found)
    }

    /// Indexed files with their text and vectors, for the graph.
    pub async fn graph_files(&self) -> Result<Vec<GraphFile>> {
        let mut files: Vec<GraphFile> = sqlx::query(
            "SELECT id, path, title, kind, links FROM files WHERE error IS NULL ORDER BY id",
        )
        .fetch_all(&self.pool)
        .await?
        .into_iter()
        .map(|r| GraphFile {
            id: r.get(0),
            path: r.get(1),
            title: r.get(2),
            kind: r.get(3),
            links: serde_json::from_str(r.get::<&str, _>(4)).unwrap_or_default(),
            text: String::new(),
            vectors: Vec::new(),
        })
        .collect();
        let rows = sqlx::query("SELECT file_id, text, embedding FROM chunks ORDER BY file_id, ord")
            .fetch_all(&self.pool)
            .await?;
        for r in rows {
            let id: i64 = r.get(0);
            let Ok(i) = files.binary_search_by_key(&id, |f| f.id) else {
                continue;
            };
            let file = &mut files[i];
            if !file.text.is_empty() {
                file.text.push('\n');
            }
            file.text.push_str(r.get(1));
            if let Some(blob) = r.get::<Option<&[u8]>, _>(2) {
                file.vectors.push(from_blob(blob));
            }
        }
        Ok(files)
    }

    pub async fn indexed_paths(&self) -> Result<Vec<String>> {
        Ok(sqlx::query_scalar("SELECT path FROM files")
            .fetch_all(&self.pool)
            .await?)
    }
}

fn to_blob(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn from_blob(b: &[u8]) -> Vec<f32> {
    b.as_chunks::<4>()
        .0
        .iter()
        .map(|c| f32::from_le_bytes(*c))
        .collect()
}
