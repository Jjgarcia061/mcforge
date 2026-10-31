use serde::Serialize;
use sysinfo::{CpuRefreshKind, MemoryRefreshKind, RefreshKind, System};

use crate::error::{Error, Result};

const GB: f64 = 1024.0 * 1024.0 * 1024.0;

#[derive(Debug, Clone, Serialize, specta::Type)]
#[serde(rename_all = "camelCase")]
pub struct Hardware {
    pub os: String,
    pub cpu: String,
    pub cores: u32,
    pub memory_gb: f64,
    pub accelerator: Accelerator,
}

/// What can run model layers fast. Apple Silicon shares system memory with the GPU.
#[derive(Debug, Clone, PartialEq, Serialize, specta::Type)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Accelerator {
    AppleSilicon,
    #[serde(rename_all = "camelCase")]
    Nvidia {
        name: String,
        vram_gb: f64,
    },
    None,
}

pub fn detect() -> Result<Hardware> {
    let sys = System::new_with_specifics(
        RefreshKind::nothing()
            .with_memory(MemoryRefreshKind::nothing().with_ram())
            .with_cpu(CpuRefreshKind::nothing()),
    )
    .map_err(|e| Error::Io(format!("cannot read system information: {e}")))?;
    let cpu = sys
        .cpus()
        .first()
        .map(|c| c.brand().trim().to_owned())
        .filter(|b| !b.is_empty())
        .unwrap_or_else(|| "Unknown CPU".into());

    Ok(Hardware {
        os: System::long_os_version().unwrap_or_else(|_| std::env::consts::OS.into()),
        cores: System::physical_core_count().unwrap_or(sys.cpus().len()) as u32,
        memory_gb: round1(sys.total_memory() as f64 / GB),
        accelerator: detect_accelerator(),
        cpu,
    })
}

fn detect_accelerator() -> Accelerator {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        return Accelerator::AppleSilicon;
    }
    nvidia().unwrap_or(Accelerator::None)
}

/// Reads the first NVIDIA GPU through `nvidia-smi`, which ships with the driver.
fn nvidia() -> Option<Accelerator> {
    let out = std::process::Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    parse_nvidia(&String::from_utf8_lossy(&out.stdout))
}

fn parse_nvidia(csv: &str) -> Option<Accelerator> {
    let (name, mib) = csv.lines().next()?.split_once(',')?;
    let mib: f64 = mib.trim().parse().ok()?;
    Some(Accelerator::Nvidia {
        name: name.trim().to_owned(),
        vram_gb: round1(mib / 1024.0),
    })
}

fn round1(v: f64) -> f64 {
    (v * 10.0).round() / 10.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_this_machine() {
        let hw = detect().unwrap();
        assert!(hw.memory_gb > 0.5);
        assert!(hw.cores >= 1);
    }

    #[test]
    fn parses_nvidia_smi_output() {
        let acc = parse_nvidia("NVIDIA GeForce RTX 4070, 12282\n").unwrap();
        assert_eq!(
            acc,
            Accelerator::Nvidia {
                name: "NVIDIA GeForce RTX 4070".into(),
                vram_gb: 12.0
            }
        );
        assert!(parse_nvidia("garbage").is_none());
    }
}
