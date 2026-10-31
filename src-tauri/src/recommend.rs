//! Ranks local models for the detected hardware. The numbers are deliberately
//! conservative heuristics; they decide labels, not hard limits.

use std::cmp::Reverse;

use serde::Serialize;

use crate::catalog::{LocalModel, ToolSkill};
use crate::hardware::{Accelerator, Hardware};

/// Below this size, models call tools unreliably no matter how well they run.
const MIN_PARAMS_FOR_TOOLS: f64 = 7.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub enum Fit {
    Smooth,
    Usable,
    TooBig,
}

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Recommendation {
    pub model: LocalModel,
    pub memory_needed_gb: f64,
    pub fit: Fit,
    pub weak_tool_use: bool,
    pub recommended: bool,
}

/// Memory a model can use while staying fast, and the most it can use at all.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Budget {
    fast_gb: f64,
    max_gb: f64,
}

impl Budget {
    fn for_hardware(hw: &Hardware) -> Self {
        let ram = hw.memory_gb;
        match &hw.accelerator {
            // Unified memory: macOS lets the GPU wire roughly 70–75% of RAM.
            Accelerator::AppleSilicon => Self {
                fast_gb: ram * 0.70,
                max_gb: ram * 0.85,
            },
            // Layers that don't fit in VRAM spill to system RAM and slow down.
            Accelerator::Nvidia { vram_gb, .. } => Self {
                fast_gb: vram_gb * 0.92,
                max_gb: vram_gb * 0.92 + ram * 0.5,
            },
            // CPU only: small models stay responsive, larger ones crawl.
            Accelerator::None => Self {
                fast_gb: (ram * 0.4).min(6.0),
                max_gb: ram * 0.6,
            },
        }
    }
}

/// Weights plus KV cache and runtime overhead for a typical 8k context.
fn memory_needed(m: &LocalModel) -> f64 {
    m.size_gb * 1.1 + 0.8
}

pub fn rank(hw: &Hardware, models: &[LocalModel]) -> Vec<Recommendation> {
    let budget = Budget::for_hardware(hw);
    let mut ranked: Vec<_> = models
        .iter()
        .map(|m| {
            let need = memory_needed(m);
            Recommendation {
                fit: if need <= budget.fast_gb {
                    Fit::Smooth
                } else if need <= budget.max_gb {
                    Fit::Usable
                } else {
                    Fit::TooBig
                },
                memory_needed_gb: (need * 10.0).round() / 10.0,
                weak_tool_use: m.params_b < MIN_PARAMS_FOR_TOOLS || m.tools < ToolSkill::Good,
                recommended: false,
                model: m.clone(),
            }
        })
        .collect();

    ranked.sort_by(|a, b| {
        (a.fit, Reverse(a.model.tools))
            .cmp(&(b.fit, Reverse(b.model.tools)))
            .then(b.model.params_b.total_cmp(&a.model.params_b))
    });

    let pick = ranked
        .iter()
        .position(|r| r.fit == Fit::Smooth && !r.weak_tool_use)
        .or_else(|| ranked.iter().position(|r| r.fit == Fit::Smooth));
    if let Some(i) = pick {
        ranked[i].recommended = true;
    }
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, params_b: f64, size_gb: f64, tools: ToolSkill) -> LocalModel {
        LocalModel {
            id: id.into(),
            name: id.into(),
            params_b,
            size_gb,
            tools,
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

    fn models() -> Vec<LocalModel> {
        vec![
            model("small", 3.0, 2.0, ToolSkill::Fair),
            model("mid", 8.0, 5.0, ToolSkill::Good),
            model("big", 21.0, 14.0, ToolSkill::Great),
            model("huge", 117.0, 65.0, ToolSkill::Great),
        ]
    }

    fn recommended(r: &[Recommendation]) -> Option<&str> {
        r.iter()
            .find(|r| r.recommended)
            .map(|r| r.model.id.as_str())
    }

    #[test]
    fn apple_24gb_gets_the_strongest_model_that_runs_fast() {
        let r = rank(&hw(24.0, Accelerator::AppleSilicon), &models());
        assert_eq!(recommended(&r), Some("big"));
        assert_eq!(r.last().unwrap().fit, Fit::TooBig);
    }

    #[test]
    fn small_vram_limits_fast_models_but_ram_allows_offload() {
        let gpu = Accelerator::Nvidia {
            name: "x".into(),
            vram_gb: 8.0,
        };
        let r = rank(&hw(32.0, gpu), &models());
        assert_eq!(recommended(&r), Some("mid"));
        assert_eq!(
            r.iter().find(|r| r.model.id == "big").unwrap().fit,
            Fit::Usable
        );
    }

    #[test]
    fn weak_machine_falls_back_to_a_small_model_and_flags_it() {
        let r = rank(&hw(8.0, Accelerator::None), &models());
        let pick = r.iter().find(|r| r.recommended).unwrap();
        assert_eq!(pick.model.id, "small");
        assert!(pick.weak_tool_use);
    }

    #[test]
    fn nothing_recommended_when_nothing_runs_fast() {
        let r = rank(&hw(2.0, Accelerator::None), &models());
        assert_eq!(recommended(&r), None);
    }
}
