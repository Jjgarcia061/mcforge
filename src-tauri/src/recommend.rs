//! Recommends local models from the hardware and the user's answers to the
//! setup interview: one model that covers every task they chose, plus the
//! library's search model when a task needs it. The numbers are deliberately
//! conservative heuristics; they decide labels and order, not hard limits.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::catalog::{EmbeddingModel, LocalModel, Models, Rating, UseCase};
use crate::hardware::{Accelerator, Hardware};

/// Below this size, models call tools unreliably no matter how well they run.
const MIN_PARAMS_FOR_TOOLS: f64 = 7.0;
/// The chat always uses tools, so tool calling always counts this much.
const TOOLS_WEIGHT: f64 = 2.0;
const SPANISH_WEIGHT: f64 = 2.0;
/// An installed model this close to the best one is recommended instead: nothing to download.
const INSTALLED_MARGIN: f64 = 0.25;
/// Room left on disk after downloading, so the system is not left full.
const DISK_MARGIN_GB: f64 = 2.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Smooth,
    Usable,
    TooBig,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Language {
    Spanish,
    English,
    #[default]
    Both,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Fast,
    #[default]
    Balanced,
    Quality,
}

/// The user's answers to the setup interview.
#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Needs {
    #[serde(default)]
    pub use_cases: Vec<String>,
    #[serde(default)]
    pub language: Language,
    #[serde(default)]
    pub priority: Priority,
}

/// Why a model suits the user, for the UI to word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Reason {
    Installed,
    FitsSmooth,
    FastMoe,
    ToolsGreat,
    AnalysisGreat,
    CodeGreat,
    SpanishGreat,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub model: LocalModel,
    pub memory_needed_gb: f64,
    pub fit: Fit,
    pub weak_tool_use: bool,
    pub installed: bool,
    /// 0 to 3: how well the model covers what the user asked for.
    pub score: f64,
    pub reasons: Vec<Reason>,
    pub recommended: bool,
}

/// What to download to cover everything the user chose.
#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Kit {
    /// The model for everything; `None` when no model runs on this machine.
    pub main: Option<String>,
    /// The best model that runs here still calls tools unreliably.
    pub weak: bool,
    /// The library's search model, when a chosen task needs it.
    pub search: Option<EmbeddingModel>,
    pub search_installed: bool,
    /// What is not installed yet.
    pub download_gb: f64,
    pub free_gb: Option<f64>,
    pub enough_space: bool,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct ModelAdvice {
    pub kit: Kit,
    /// Every model, best for the user first.
    pub ranked: Vec<Recommendation>,
}

/// Memory a model can use while staying fast, and the most it can use at all.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Capacity {
    pub fast_gb: f64,
    pub max_gb: f64,
}

impl Capacity {
    pub fn for_hardware(hw: &Hardware) -> Self {
        let ram = hw.memory_gb;
        let (fast_gb, max_gb) = match &hw.accelerator {
            // Unified memory: macOS lets the GPU wire roughly 70–75% of RAM.
            Accelerator::AppleSilicon => (ram * 0.70, ram * 0.85),
            // Layers that don't fit in VRAM spill to system RAM and slow down.
            Accelerator::Nvidia { vram_gb, .. } => (vram_gb * 0.92, vram_gb * 0.92 + ram * 0.5),
            // CPU only: small models stay responsive, larger ones crawl.
            Accelerator::None => ((ram * 0.4).min(6.0), ram * 0.6),
        };
        Self { fast_gb, max_gb }
    }

    fn fit(self, need_gb: f64) -> Fit {
        if need_gb <= self.fast_gb {
            Fit::Smooth
        } else if need_gb <= self.max_gb {
            Fit::Usable
        } else {
            Fit::TooBig
        }
    }
}

/// Weights plus KV cache and runtime overhead for a typical 8k context.
fn memory_needed(m: &LocalModel) -> f64 {
    m.size_gb * 1.1 + 0.8
}

/// Whether any model of the catalog runs smoothly on this machine.
pub fn runs_any(hw: &Hardware, models: &[LocalModel]) -> bool {
    let capacity = Capacity::for_hardware(hw);
    models
        .iter()
        .any(|m| capacity.fit(memory_needed(m)) == Fit::Smooth)
}

/// How much each skill matters for these answers.
fn weights(needs: &Needs, use_cases: &[UseCase]) -> BTreeMap<String, f64> {
    let mut weights = BTreeMap::from([("tools".to_owned(), TOOLS_WEIGHT)]);
    for u in use_cases.iter().filter(|u| needs.use_cases.contains(&u.id)) {
        for (skill, weight) in &u.skills {
            *weights.entry(skill.clone()).or_default() += weight;
        }
    }
    if needs.language != Language::English {
        *weights.entry("spanish".into()).or_default() += SPANISH_WEIGHT;
    }
    weights
}

fn value(r: Rating) -> f64 {
    f64::from(r as u8)
}

fn score(m: &LocalModel, weights: &BTreeMap<String, f64>) -> f64 {
    let total: f64 = weights.values().sum();
    weights
        .iter()
        .map(|(skill, w)| w * value(m.rating(skill)))
        .sum::<f64>()
        / total
}

/// The score, nudged toward speed or size as the user prefers.
fn preference(r: &Recommendation, priority: Priority) -> f64 {
    let active = r.model.active_params_b.unwrap_or(r.model.params_b);
    match priority {
        Priority::Fast => r.score - 0.06 * active,
        Priority::Balanced => r.score - 0.01 * active,
        Priority::Quality => r.score + 0.01 * r.model.params_b,
    }
}

fn reasons(
    m: &LocalModel,
    fit: Fit,
    installed: bool,
    weights: &BTreeMap<String, f64>,
) -> Vec<Reason> {
    let mut reasons = Vec::new();
    if installed {
        reasons.push(Reason::Installed);
    }
    if fit == Fit::Smooth {
        reasons.push(Reason::FitsSmooth);
    }
    if m.active_params_b.is_some() && fit != Fit::TooBig {
        reasons.push(Reason::FastMoe);
    }
    for (skill, reason) in [
        ("tools", Reason::ToolsGreat),
        ("analysis", Reason::AnalysisGreat),
        ("code", Reason::CodeGreat),
        ("spanish", Reason::SpanishGreat),
    ] {
        if weights.contains_key(skill) && m.rating(skill) == Rating::Great {
            reasons.push(reason);
        }
    }
    reasons
}

fn round1(x: f64) -> f64 {
    (x * 10.0).round() / 10.0
}

pub fn advise(
    hw: &Hardware,
    models: &Models,
    use_cases: &[UseCase],
    needs: &Needs,
    installed: &[String],
    free_gb: Option<f64>,
) -> ModelAdvice {
    let capacity = Capacity::for_hardware(hw);
    let weights = weights(needs, use_cases);
    let has = |id: &str| {
        installed
            .iter()
            .any(|m| m == id || m.strip_suffix(":latest") == Some(id))
    };

    let mut ranked: Vec<Recommendation> = models
        .local
        .iter()
        .map(|m| {
            let need = memory_needed(m);
            let fit = capacity.fit(need);
            let installed = has(&m.id);
            Recommendation {
                memory_needed_gb: round1(need),
                fit,
                weak_tool_use: m.params_b < MIN_PARAMS_FOR_TOOLS || m.tools < Rating::Good,
                installed,
                score: (score(m, &weights) * 100.0).round() / 100.0,
                reasons: reasons(m, fit, installed, &weights),
                recommended: false,
                model: m.clone(),
            }
        })
        .collect();

    // Fast and balanced want a model that runs smoothly; quality accepts a slower one.
    let allowed = |r: &Recommendation| match needs.priority {
        Priority::Quality => r.fit != Fit::TooBig,
        Priority::Fast | Priority::Balanced => r.fit == Fit::Smooth,
    };
    ranked.sort_by(|a, b| {
        allowed(b)
            .cmp(&allowed(a))
            .then(preference(b, needs.priority).total_cmp(&preference(a, needs.priority)))
            .then(a.model.id.cmp(&b.model.id))
    });

    let best = ranked
        .iter()
        .position(|r| allowed(r) && !r.weak_tool_use)
        .or_else(|| ranked.iter().position(allowed));
    let pick = best.map(|b| {
        let (best_score, best_weak) = (ranked[b].score, ranked[b].weak_tool_use);
        ranked
            .iter()
            .position(|r| {
                r.installed
                    && allowed(r)
                    && (best_weak || !r.weak_tool_use)
                    && r.score >= best_score - INSTALLED_MARGIN
            })
            .unwrap_or(b)
    });
    if let Some(i) = pick {
        ranked[i].recommended = true;
    }

    let needs_search = use_cases
        .iter()
        .any(|u| u.embedding && needs.use_cases.contains(&u.id));
    let search = needs_search
        .then(|| models.embedding.first().cloned())
        .flatten();
    let search_installed = search.as_ref().is_some_and(|s| has(&s.id));
    let main = pick.map(|i| &ranked[i]);
    let download_gb = round1(
        main.filter(|r| !r.installed)
            .map_or(0.0, |r| r.model.size_gb)
            + search
                .as_ref()
                .filter(|_| !search_installed)
                .map_or(0.0, |s| s.size_gb),
    );
    let kit = Kit {
        main: main.map(|r| r.model.id.clone()),
        weak: main.is_some_and(|r| r.weak_tool_use),
        search,
        search_installed,
        download_gb,
        free_gb: free_gb.map(round1),
        enough_space: free_gb
            .is_none_or(|free| download_gb == 0.0 || free >= download_gb + DISK_MARGIN_GB),
    };
    ModelAdvice { kit, ranked }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Rating::*;

    fn model(
        id: &str,
        params_b: f64,
        size_gb: f64,
        [tools, analysis, code, spanish]: [Rating; 4],
        active_params_b: Option<f64>,
    ) -> LocalModel {
        LocalModel {
            id: id.into(),
            name: id.into(),
            params_b,
            size_gb,
            tools,
            analysis,
            code,
            spanish,
            active_params_b,
        }
    }

    fn hw(memory_gb: f64, accelerator: Accelerator) -> Hardware {
        Hardware {
            os: "test".into(),
            cpu: "test".into(),
            cores: 8,
            memory_gb,
            accelerator,
        }
    }

    fn catalog() -> Models {
        Models {
            local: vec![
                model("small", 3.0, 2.0, [Fair, Fair, Poor, Fair], None),
                model("coder", 8.0, 5.0, [Good, Good, Great, Fair], None),
                model("polyglot", 14.0, 9.0, [Good, Good, Good, Great], None),
                model("moe", 30.0, 15.5, [Great, Great, Great, Great], Some(3.3)),
                model("huge", 117.0, 65.0, [Great, Great, Great, Great], Some(5.1)),
            ],
            api: Vec::new(),
            embedding: vec![EmbeddingModel {
                id: "embed:0.6b".into(),
                name: "Embed".into(),
                size_gb: 0.6,
                query_prefix: String::new(),
            }],
        }
    }

    fn use_cases() -> Vec<UseCase> {
        let case = |id: &str, skills: &[(&str, f64)], embedding: bool| UseCase {
            id: id.into(),
            connectors: Vec::new(),
            skills: skills.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
            embedding,
        };
        vec![
            case("documents", &[("analysis", 1.0)], true),
            case("development", &[("code", 2.0)], false),
        ]
    }

    fn needs(use_cases: &[&str], language: Language, priority: Priority) -> Needs {
        Needs {
            use_cases: use_cases.iter().map(|s| (*s).to_owned()).collect(),
            language,
            priority,
        }
    }

    fn advise_on(
        memory_gb: f64,
        needs: &Needs,
        installed: &[&str],
        free: Option<f64>,
    ) -> ModelAdvice {
        let installed: Vec<String> = installed.iter().map(|s| (*s).to_owned()).collect();
        advise(
            &hw(memory_gb, Accelerator::AppleSilicon),
            &catalog(),
            &use_cases(),
            needs,
            &installed,
            free,
        )
    }

    #[test]
    fn spanish_documents_get_the_polyglot_and_the_search_model() {
        let a = advise_on(
            16.0,
            &needs(&["documents"], Language::Spanish, Priority::Balanced),
            &[],
            Some(100.0),
        );
        assert_eq!(
            a.kit.main.as_deref(),
            Some("polyglot"),
            "the moe needs more than runs smoothly here"
        );
        assert_eq!(
            a.kit.search.as_ref().map(|s| s.id.as_str()),
            Some("embed:0.6b")
        );
        assert_eq!(a.kit.download_gb, 9.6);
        assert!(a.kit.enough_space);
        let polyglot = a.ranked.iter().find(|r| r.model.id == "polyglot").unwrap();
        assert!(polyglot.reasons.contains(&Reason::SpanishGreat));
        assert!(polyglot.recommended);
    }

    #[test]
    fn english_code_prefers_the_coder_and_needs_no_search_model() {
        let a = advise_on(
            16.0,
            &needs(&["development"], Language::English, Priority::Balanced),
            &[],
            None,
        );
        assert_eq!(a.kit.main.as_deref(), Some("coder"));
        assert!(a.kit.search.is_none());
    }

    #[test]
    fn quality_accepts_a_slower_model_and_fast_prefers_light_ones() {
        let quality = advise_on(
            24.0,
            &needs(&[], Language::Both, Priority::Quality),
            &[],
            None,
        );
        assert_eq!(quality.kit.main.as_deref(), Some("moe"));
        let fast = advise_on(
            12.0,
            &needs(&[], Language::English, Priority::Fast),
            &[],
            None,
        );
        assert_eq!(
            fast.kit.main.as_deref(),
            Some("coder"),
            "the lightest reliable model"
        );
    }

    #[test]
    fn an_installed_model_close_to_the_best_saves_the_download() {
        // Quality ranks the polyglot first, but the installed coder scores the same.
        let a = advise_on(
            16.0,
            &needs(&["documents"], Language::English, Priority::Quality),
            &["coder:latest", "embed:0.6b"],
            Some(1.0),
        );
        assert_eq!(a.ranked[0].model.id, "polyglot");
        assert_eq!(a.kit.main.as_deref(), Some("coder"));
        assert!(a.kit.search_installed);
        assert_eq!(a.kit.download_gb, 0.0);
        assert!(a.kit.enough_space, "nothing to download needs no space");
    }

    #[test]
    fn weak_machines_and_short_disks_are_flagged() {
        let a = advise(
            &hw(8.0, Accelerator::None),
            &catalog(),
            &use_cases(),
            &needs(&["documents"], Language::Spanish, Priority::Balanced),
            &[],
            Some(1.5),
        );
        assert_eq!(a.kit.main.as_deref(), Some("small"));
        assert!(a.kit.weak);
        assert!(!a.kit.enough_space);
        let none = advise(
            &hw(2.0, Accelerator::None),
            &catalog(),
            &use_cases(),
            &Needs::default(),
            &[],
            None,
        );
        assert_eq!(none.kit.main, None);
        assert!(!runs_any(&hw(2.0, Accelerator::None), &catalog().local));
    }

    #[test]
    fn small_vram_limits_fast_models_but_ram_allows_offload() {
        let gpu = Accelerator::Nvidia {
            name: "x".into(),
            vram_gb: 8.0,
        };
        let a = advise(
            &hw(32.0, gpu),
            &catalog(),
            &use_cases(),
            &Needs::default(),
            &[],
            None,
        );
        assert_eq!(a.kit.main.as_deref(), Some("coder"));
        let polyglot = a.ranked.iter().find(|r| r.model.id == "polyglot").unwrap();
        assert_eq!(polyglot.fit, Fit::Usable);
    }
}
