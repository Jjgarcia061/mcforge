//! Hybrid search over passages: full-text (BM25) and vector similarity, fused
//! by rank. Either half works alone, so the library stays searchable without
//! an embedding model.

use std::collections::HashMap;

use super::Embedder;
use super::index::{Index, Passage};
use crate::error::Result;

/// Candidates each half contributes before fusion.
const CANDIDATES: usize = 40;
/// Reciprocal rank fusion constant; 60 is the usual choice.
const RRF_K: f64 = 60.0;
/// So one long document cannot fill every slot.
const PER_FILE: usize = 2;

/// Words that say nothing about a topic, in Spanish and English.
const STOPWORDS: &[&str] = &[
    "a", "al", "algo", "algunas", "algunos", "ante", "antes", "aquí", "así", "cada", "como", "con",
    "contra", "cual", "cuando", "de", "del", "desde", "donde", "dos", "durante", "el", "ella",
    "ellas", "ellos", "en", "entre", "era", "es", "esa", "esas", "ese", "eso", "esos", "esta",
    "está", "están", "estas", "este", "esto", "estos", "fue", "ha", "han", "hasta", "hay", "la",
    "las", "le", "les", "lo", "los", "más", "me", "mi", "muy", "ni", "no", "nos", "o", "otra",
    "otras", "otro", "otros", "para", "pero", "poco", "por", "porque", "que", "qué", "quien", "se",
    "ser", "si", "sí", "sin", "sobre", "son", "su", "sus", "también", "tan", "te", "tiene",
    "tienen", "todo", "todos", "tu", "un", "una", "unas", "uno", "unos", "usted", "y", "ya",
    "about", "after", "all", "also", "an", "and", "any", "are", "as", "at", "be", "been", "but",
    "by", "can", "do", "does", "for", "from", "had", "has", "have", "how", "if", "in", "into",
    "is", "it", "its", "may", "more", "must", "not", "of", "on", "or", "our", "out", "should",
    "so", "than", "that", "the", "their", "them", "then", "there", "these", "they", "this", "to",
    "was", "we", "were", "what", "when", "which", "who", "will", "with", "would", "you", "your",
];

/// Lowercase words worth matching: letters or digits, two or more, not stopwords.
pub fn terms(text: &str) -> impl Iterator<Item = String> + '_ {
    text.split(|c: char| !c.is_alphanumeric())
        .filter(|w| w.chars().count() >= 2)
        .map(str::to_lowercase)
        .filter(|w| !STOPWORDS.contains(&w.as_str()))
}

/// Every passage vector in one contiguous, normalized block.
pub struct Vectors {
    chunks: Vec<i64>,
    dims: usize,
    data: Vec<f32>,
}

impl Vectors {
    pub fn new(rows: Vec<(i64, i64, Vec<f32>)>) -> Self {
        let dims = rows.first().map_or(0, |(_, _, v)| v.len());
        let mut chunks = Vec::with_capacity(rows.len());
        let mut data = Vec::with_capacity(rows.len() * dims);
        for (chunk, _, v) in rows.into_iter().filter(|(_, _, v)| v.len() == dims) {
            chunks.push(chunk);
            data.extend(normalized(v));
        }
        Self { chunks, dims, data }
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    /// Passage ids by cosine similarity to `query`, best first.
    fn nearest(&self, query: &[f32], n: usize) -> Vec<i64> {
        if query.len() != self.dims {
            return Vec::new();
        }
        let q = normalized(query.to_vec());
        let mut scored: Vec<(f32, i64)> = self
            .data
            .chunks_exact(self.dims)
            .zip(&self.chunks)
            .map(|(v, &id)| (dot(v, &q), id))
            .collect();
        scored.sort_by(|a, b| b.0.total_cmp(&a.0));
        scored.into_iter().take(n).map(|(_, id)| id).collect()
    }
}

pub fn normalized(mut v: Vec<f32>) -> Vec<f32> {
    let norm = dot(&v, &v).sqrt();
    if norm > 0.0 {
        v.iter_mut().for_each(|x| *x /= norm);
    }
    v
}

pub fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The `k` passages that best match `query`.
pub async fn search(
    index: &Index,
    vectors: Option<&Vectors>,
    embedder: Option<&dyn Embedder>,
    query: &str,
    k: usize,
) -> Result<Vec<Passage>> {
    let words: Vec<String> = terms(query).collect();
    let keyword = index.keyword(&words, CANDIDATES as u32).await?;
    let semantic = match (vectors.filter(|v| !v.is_empty()), embedder) {
        (Some(vectors), Some(embedder)) => {
            // A failing embedder costs the semantic half, not the search.
            let query = [query.to_owned()];
            match embedder.embed(&query, true).await {
                Ok(mut v) if !v.is_empty() => vectors.nearest(&v.swap_remove(0), CANDIDATES),
                _ => Vec::new(),
            }
        }
        _ => Vec::new(),
    };

    let mut scores: HashMap<i64, f64> = HashMap::new();
    for ranking in [&keyword, &semantic] {
        for (rank, id) in ranking.iter().enumerate() {
            *scores.entry(*id).or_default() += 1.0 / (RRF_K + rank as f64 + 1.0);
        }
    }
    let mut ranked: Vec<(i64, f64)> = scores.into_iter().collect();
    ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let ids: Vec<i64> = ranked.into_iter().map(|(id, _)| id).collect();

    let mut per_file: HashMap<i64, usize> = HashMap::new();
    Ok(index
        .passages(&ids)
        .await?
        .into_iter()
        .filter(|p| {
            let n = per_file.entry(p.file).or_default();
            *n += 1;
            *n <= PER_FILE
        })
        .take(k)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terms_drop_stopwords_and_punctuation() {
        let t: Vec<String> = terms("¿Cuál es la política de descuentos para el 2026?").collect();
        assert_eq!(t, ["cuál", "política", "descuentos", "2026"]);
    }

    #[test]
    fn nearest_vectors_come_first() {
        let v = Vectors::new(vec![
            (1, 1, vec![1.0, 0.0]),
            (2, 1, vec![0.0, 3.0]),
            (3, 2, vec![0.7, 0.7]),
        ]);
        assert_eq!(v.nearest(&[0.0, 1.0], 2), [2, 3]);
        assert!(
            v.nearest(&[1.0], 2).is_empty(),
            "wrong dimensions find nothing"
        );
    }
}
