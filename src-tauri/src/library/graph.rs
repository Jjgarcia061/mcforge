//! The map of a library: one node per file, joined to the files it is most
//! similar to and to the ones it links, grouped into topics (Louvain) named
//! after their most distinctive words.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::index::GraphFile;
use super::search::{dot, normalized, terms};

/// Each file keeps its closest neighbours only, so the map stays readable.
const NEIGHBOURS: usize = 4;
/// Below these similarities two files are not considered related.
const MIN_SEMANTIC: f32 = 0.5;
const MIN_WORDS: f32 = 0.06;
/// Words kept per file for the word-based similarity and the topic names.
const TOP_TERMS: usize = 64;
const MAX_TEXT_BYTES: usize = 40_000;
pub const MAX_NODES: usize = 1500;

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Graph {
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    pub clusters: Vec<Cluster>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Node {
    pub title: String,
    pub path: String,
    pub kind: String,
    /// Index into `clusters`.
    pub cluster: u32,
    pub degree: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Edge {
    /// Indexes into `nodes`.
    pub source: u32,
    pub target: u32,
    pub weight: f32,
    pub kind: EdgeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum EdgeKind {
    Similar,
    Link,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, specta::Type)]
pub struct Cluster {
    /// Its two most distinctive words; empty for a file on its own.
    pub label: String,
    pub size: u32,
}

pub fn build(files: &[GraphFile]) -> Graph {
    let n = files.len();
    let (words, vocabulary) = word_vectors(files);
    // Vectors from the embedding model when every file has them; shared words otherwise.
    let semantic: Option<Vec<Vec<f32>>> = files
        .iter()
        .map(|f| mean(&f.vectors))
        .collect::<Option<Vec<_>>>()
        .filter(|v| !v.is_empty());
    let similarity = |i: usize, j: usize| match &semantic {
        Some(v) => dot(&v[i], &v[j]),
        None => sparse_dot(&words[i], &words[j]),
    };
    let min = if semantic.is_some() {
        MIN_SEMANTIC
    } else {
        MIN_WORDS
    };

    let mut weights: HashMap<(usize, usize), (f32, EdgeKind)> = HashMap::new();
    for i in 0..n {
        let mut near: Vec<(f32, usize)> = (0..n)
            .filter(|&j| j != i)
            .map(|j| (similarity(i, j), j))
            .filter(|&(s, _)| s >= min)
            .collect();
        near.sort_by(|a, b| b.0.total_cmp(&a.0));
        for (s, j) in near.into_iter().take(NEIGHBOURS) {
            weights.insert((i.min(j), i.max(j)), (s, EdgeKind::Similar));
        }
    }
    for (i, j) in links(files) {
        weights.insert((i.min(j), i.max(j)), (1.0, EdgeKind::Link));
    }
    let mut edges: Vec<(usize, usize, f32, EdgeKind)> = weights
        .into_iter()
        .map(|((a, b), (w, k))| (a, b, w, k))
        .collect();
    edges.sort_by_key(|&(a, b, ..)| (a, b));

    let community = louvain(
        n,
        &edges
            .iter()
            .map(|&(a, b, w, _)| (a, b, f64::from(w)))
            .collect::<Vec<_>>(),
    );
    let mut degree = vec![0u32; n];
    for &(a, b, ..) in &edges {
        degree[a] += 1;
        degree[b] += 1;
    }

    // Keep the most connected files when there are too many to draw.
    let mut keep: Vec<usize> = (0..n).collect();
    keep.sort_by_key(|&i| (std::cmp::Reverse(degree[i]), i));
    keep.truncate(MAX_NODES);
    keep.sort_unstable();
    let position: HashMap<usize, u32> = keep
        .iter()
        .enumerate()
        .map(|(p, &i)| (i, p as u32))
        .collect();

    // Topics numbered by size, biggest first.
    let mut members: HashMap<usize, Vec<usize>> = HashMap::new();
    for &i in &keep {
        members.entry(community[i]).or_default().push(i);
    }
    let mut topics: Vec<Vec<usize>> = members.into_values().collect();
    topics.sort_by_key(|m| (std::cmp::Reverse(m.len()), m[0]));
    let mut topic_of = vec![0u32; n];
    for (t, m) in topics.iter().enumerate() {
        for &i in m {
            topic_of[i] = t as u32;
        }
    }

    Graph {
        nodes: keep
            .iter()
            .map(|&i| Node {
                title: files[i].title.clone(),
                path: files[i].path.clone(),
                kind: files[i].kind.clone(),
                cluster: topic_of[i],
                degree: degree[i],
            })
            .collect(),
        edges: edges
            .iter()
            .filter_map(|&(a, b, weight, kind)| {
                Some(Edge {
                    source: *position.get(&a)?,
                    target: *position.get(&b)?,
                    weight,
                    kind,
                })
            })
            .collect(),
        clusters: topics
            .iter()
            .map(|m| Cluster {
                label: if m.len() > 1 {
                    label(m, &words, &vocabulary)
                } else {
                    String::new()
                },
                size: m.len() as u32,
            })
            .collect(),
    }
}

fn mean(vectors: &[Vec<f32>]) -> Option<Vec<f32>> {
    let first = vectors.first()?;
    let dims = first.len();
    let mut sum = vec![0.0; dims];
    for v in vectors.iter().filter(|v| v.len() == dims) {
        sum.iter_mut().zip(v).for_each(|(s, x)| *s += x);
    }
    Some(normalized(sum))
}

/// Each file as its `TOP_TERMS` heaviest TF-IDF words, sorted by word id.
type Sparse = Vec<(u32, f32)>;

fn word_vectors(files: &[GraphFile]) -> (Vec<Sparse>, Vec<String>) {
    let mut vocabulary: Vec<String> = Vec::new();
    let mut ids: HashMap<String, u32> = HashMap::new();
    let counts: Vec<HashMap<u32, f32>> = files
        .iter()
        .map(|f| {
            let text = &f.text[..f.text.floor_char_boundary(MAX_TEXT_BYTES)];
            let mut counts: HashMap<u32, f32> = HashMap::new();
            for word in
                terms(text).filter(|w| w.chars().count() >= 3 && !w.chars().all(|c| c.is_numeric()))
            {
                let id = *ids.entry(word).or_insert_with_key(|w| {
                    vocabulary.push(w.clone());
                    (vocabulary.len() - 1) as u32
                });
                *counts.entry(id).or_default() += 1.0;
            }
            counts
        })
        .collect();
    let mut df: HashMap<u32, f32> = HashMap::new();
    for c in &counts {
        for &id in c.keys() {
            *df.entry(id).or_default() += 1.0;
        }
    }
    let n = files.len() as f32;
    let vectors = counts
        .into_iter()
        .map(|c| {
            let total: f32 = c.values().sum::<f32>().max(1.0);
            let mut v: Sparse = c
                .into_iter()
                .map(|(id, count)| (id, count / total * (n / df[&id]).ln()))
                .filter(|&(_, w)| w > 0.0)
                .collect();
            v.sort_by(|a, b| b.1.total_cmp(&a.1));
            v.truncate(TOP_TERMS);
            let norm = v.iter().map(|(_, w)| w * w).sum::<f32>().sqrt();
            v.iter_mut()
                .for_each(|(_, w)| *w /= norm.max(f32::MIN_POSITIVE));
            v.sort_by_key(|&(id, _)| id);
            v
        })
        .collect();
    (vectors, vocabulary)
}

fn sparse_dot(a: &Sparse, b: &Sparse) -> f32 {
    let (mut i, mut j, mut sum) = (0, 0, 0.0);
    while i < a.len() && j < b.len() {
        match a[i].0.cmp(&b[j].0) {
            std::cmp::Ordering::Less => i += 1,
            std::cmp::Ordering::Greater => j += 1,
            std::cmp::Ordering::Equal => {
                sum += a[i].1 * b[j].1;
                i += 1;
                j += 1;
            }
        }
    }
    sum
}

/// The two words that weigh most across a topic's files.
fn label(members: &[usize], words: &[Sparse], vocabulary: &[String]) -> String {
    let mut weight: HashMap<u32, f32> = HashMap::new();
    for &i in members {
        for &(id, w) in &words[i] {
            if vocabulary[id as usize].chars().count() >= 4 {
                *weight.entry(id).or_default() += w;
            }
        }
    }
    let mut top: Vec<(u32, f32)> = weight.into_iter().collect();
    top.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    top.iter()
        .take(2)
        .map(|&(id, _)| vocabulary[id as usize].as_str())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Pairs of files joined by a Markdown link: `[[Name]]` matches a file name,
/// a relative path matches a file next to the one that links it.
fn links(files: &[GraphFile]) -> Vec<(usize, usize)> {
    let stem = |p: &str| {
        Path::new(p)
            .file_stem()
            .map(|s| s.to_string_lossy().to_lowercase())
            .unwrap_or_default()
    };
    let by_name: HashMap<String, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (stem(&f.path), i))
        .collect();
    let by_path: HashMap<PathBuf, usize> = files
        .iter()
        .enumerate()
        .map(|(i, f)| (PathBuf::from(&f.path), i))
        .collect();
    let mut out = Vec::new();
    for (i, f) in files.iter().enumerate() {
        let dir = Path::new(&f.path).parent().unwrap_or(Path::new(""));
        for target in &f.links {
            let relative = lexical(&dir.join(target));
            let j = by_path
                .get(&relative)
                .or_else(|| by_path.get(&relative.with_extension("md")))
                .or_else(|| by_name.get(&stem(target)));
            if let Some(&j) = j.filter(|&&j| j != i) {
                out.push((i, j));
            }
        }
    }
    out
}

/// `a/b/../c` as `a/c`, without touching the disk.
fn lexical(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

/// Communities that maximize modularity: nodes move to the neighbouring
/// community that gains most, then communities become nodes, until stable.
fn louvain(n: usize, edges: &[(usize, usize, f64)]) -> Vec<usize> {
    let mut community: Vec<usize> = (0..n).collect();
    let mut adjacency: Vec<HashMap<usize, f64>> = vec![HashMap::new(); n];
    for &(a, b, w) in edges {
        *adjacency[a].entry(b).or_default() += w;
        *adjacency[b].entry(a).or_default() += w;
    }
    loop {
        let (moved, part) = local_moves(&adjacency);
        if !moved {
            return community;
        }
        for c in community.iter_mut() {
            *c = part[*c];
        }
        let groups = part.iter().max().map_or(0, |m| m + 1);
        let mut merged: Vec<HashMap<usize, f64>> = vec![HashMap::new(); groups];
        for (i, neighbours) in adjacency.iter().enumerate() {
            for (&j, &w) in neighbours {
                *merged[part[i]].entry(part[j]).or_default() += w;
            }
        }
        adjacency = merged;
    }
}

/// One Louvain pass. Self-loops (`adjacency[i][i]`) carry a community's inner weight.
fn local_moves(adjacency: &[HashMap<usize, f64>]) -> (bool, Vec<usize>) {
    let n = adjacency.len();
    let degree: Vec<f64> = adjacency.iter().map(|a| a.values().sum()).collect();
    let m2: f64 = degree.iter().sum();
    let mut community: Vec<usize> = (0..n).collect();
    if m2 == 0.0 {
        return (false, community);
    }
    let mut total = degree.clone();
    let mut moved = false;
    loop {
        let mut improved = false;
        for i in 0..n {
            let current = community[i];
            let mut towards: HashMap<usize, f64> = HashMap::new();
            for (&j, &w) in &adjacency[i] {
                if j != i {
                    *towards.entry(community[j]).or_default() += w;
                }
            }
            total[current] -= degree[i];
            let gain = |c: usize, w: f64| w - total[c] * degree[i] / m2;
            let mut best = (
                current,
                gain(current, towards.get(&current).copied().unwrap_or(0.0)),
            );
            let mut candidates: Vec<(usize, f64)> = towards.into_iter().collect();
            candidates.sort_by_key(|&(c, _)| c);
            for (c, w) in candidates {
                let g = gain(c, w);
                if g > best.1 + 1e-12 {
                    best = (c, g);
                }
            }
            total[best.0] += degree[i];
            if best.0 != current {
                community[i] = best.0;
                improved = true;
                moved = true;
            }
        }
        if !improved {
            break;
        }
    }
    // Renumber 0..k in order of first appearance.
    let mut ids: HashMap<usize, usize> = HashMap::new();
    let part = community
        .iter()
        .map(|c| {
            let next = ids.len();
            *ids.entry(*c).or_insert(next)
        })
        .collect();
    (moved, part)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str, vector: Option<[f32; 3]>, links: &[&str]) -> GraphFile {
        GraphFile {
            id: 0,
            path: path.into(),
            title: Path::new(path)
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .into_owned(),
            kind: "md".into(),
            links: links.iter().map(|l| (*l).to_owned()).collect(),
            text: text.into(),
            vectors: vector.map(|v| vec![v.to_vec()]).unwrap_or_default(),
        }
    }

    #[test]
    fn similar_files_form_topics_and_links_join_them() {
        let sales = "ventas clientes descuentos precios región vendedores metas";
        let people = "vacaciones contratación empleados nómina capacitación desempeño";
        let files = vec![
            file(
                "/b/metas.md",
                sales,
                Some([1.0, 0.1, 0.0]),
                &["../b/nomina.md"],
            ),
            file("/b/descuentos.md", sales, Some([0.9, 0.2, 0.0]), &[]),
            file("/b/precios.md", sales, Some([0.95, 0.0, 0.1]), &[]),
            file(
                "/b/vacaciones.md",
                people,
                Some([0.0, 1.0, 0.1]),
                &["Contratación", "Metas"],
            ),
            file("/b/contratación.md", people, Some([0.1, 0.9, 0.0]), &[]),
            file("/b/nomina.md", people, Some([0.0, 0.95, 0.2]), &[]),
        ];
        let g = build(&files);
        assert_eq!(g.nodes.len(), 6);
        let topic = |name: &str| g.nodes.iter().find(|n| n.title == name).unwrap().cluster;
        assert_eq!(topic("metas"), topic("precios"));
        assert_eq!(topic("vacaciones"), topic("nomina"));
        assert_ne!(topic("metas"), topic("vacaciones"));
        assert_eq!(g.clusters.len(), 2);
        assert!(
            g.clusters
                .iter()
                .all(|c| c.size == 3 && !c.label.is_empty()),
            "{:?}",
            g.clusters
        );

        let link = |a: &str, b: &str| {
            let (a, b) = (
                g.nodes.iter().position(|n| n.title == a).unwrap() as u32,
                g.nodes.iter().position(|n| n.title == b).unwrap() as u32,
            );
            g.edges.iter().any(|e| {
                e.kind == EdgeKind::Link
                    && ((e.source, e.target) == (a, b) || (e.source, e.target) == (b, a))
            })
        };
        assert!(link("metas", "nomina"), "relative path");
        assert!(link("vacaciones", "contratación"), "[[name]]");
        assert!(link("vacaciones", "metas"), "bare name");
    }

    #[test]
    fn without_vectors_shared_words_still_relate_files() {
        let files = vec![
            file(
                "/a/inventario.md",
                "inventario almacén stock rotación proveedores entradas",
                None,
                &[],
            ),
            file(
                "/a/almacen.md",
                "almacén inventario conteo stock bodega",
                None,
                &[],
            ),
            file(
                "/a/viajes.md",
                "viáticos hotel vuelos reembolso gastos",
                None,
                &[],
            ),
        ];
        let g = build(&files);
        assert_eq!(g.edges.len(), 1);
        assert_eq!(g.nodes[2].degree, 0, "unrelated files stay apart");
        assert_eq!(
            g.clusters[g.nodes[2].cluster as usize].label, "",
            "a file alone has no topic name"
        );
    }
}
