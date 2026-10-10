//! node-v1.2.7: what CPU this machine has, for the panel's node detail drawer.
//!
//! Read once, on first use: the model and the CPU count do not change while
//! the node runs, so there is no point re-reading /proc/cpuinfo every report.

use std::sync::OnceLock;

/// Longest model string sent to the panel. Real ones are well under 64.
const MAX_MODEL_LEN: usize = 128;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CpuInfo {
    /// e.g. "AMD EPYC 7B13" or "ARM Neoverse-N1"; None when the machine names
    /// none we recognise.
    pub model: Option<String>,
    /// Logical CPUs (what `nproc` counts on an unrestricted machine).
    pub cores: Option<u32>,
}

/// This machine's CPU, read from /proc/cpuinfo the first time it is asked for.
/// Without /proc/cpuinfo (not Linux) only the CPU count is known.
pub fn cpu_info() -> &'static CpuInfo {
    static INFO: OnceLock<CpuInfo> = OnceLock::new();
    INFO.get_or_init(|| {
        let mut info = std::fs::read_to_string("/proc/cpuinfo")
            .map(|text| parse(&text))
            .unwrap_or_default();
        if info.cores.is_none() {
            info.cores = std::thread::available_parallelism()
                .ok()
                .and_then(|n| u32::try_from(n.get()).ok());
        }
        info
    })
}

/// Parse /proc/cpuinfo.
///
/// x86 (and 32-bit ARM) name the CPU in `model name`. Most arm64 kernels do
/// not: they only give `CPU implementer` / `CPU part` codes, which are looked
/// up in a short table of the cores cloud ARM machines actually use. Each
/// `processor` entry is one logical CPU.
pub fn parse(cpuinfo: &str) -> CpuInfo {
    let mut model = None;
    let mut implementer = None;
    let mut part = None;
    let mut cores = 0u32;
    for line in cpuinfo.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "processor" => cores += 1,
            "model name" if model.is_none() && !value.is_empty() => model = Some(clean(value)),
            "CPU implementer" if implementer.is_none() => implementer = parse_hex(value),
            "CPU part" if part.is_none() => part = parse_hex(value),
            _ => {}
        }
    }
    let model = model.or_else(|| implementer.and_then(|i| arm_core_name(i, part)));
    CpuInfo {
        model,
        cores: (cores > 0).then_some(cores),
    }
}

/// Collapse whitespace runs (cpuinfo pads some models with spaces), drop
/// control characters and cap the length.
fn clean(raw: &str) -> String {
    let joined = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .filter(|c| !c.is_control())
        .collect::<String>();
    joined.chars().take(MAX_MODEL_LEN).collect()
}

fn parse_hex(value: &str) -> Option<u32> {
    u32::from_str_radix(value.trim_start_matches("0x"), 16).ok()
}

/// The cores behind most cloud ARM machines (names as `lscpu` prints them).
/// A known vendor with an unlisted part still says whose it is.
fn arm_core_name(implementer: u32, part: Option<u32>) -> Option<String> {
    let name = match (implementer, part) {
        (0x41, Some(0xd03)) => "ARM Cortex-A53",
        (0x41, Some(0xd04)) => "ARM Cortex-A35",
        (0x41, Some(0xd05)) => "ARM Cortex-A55",
        (0x41, Some(0xd07)) => "ARM Cortex-A57",
        (0x41, Some(0xd08)) => "ARM Cortex-A72",
        (0x41, Some(0xd09)) => "ARM Cortex-A73",
        (0x41, Some(0xd0a)) => "ARM Cortex-A75",
        (0x41, Some(0xd0b)) => "ARM Cortex-A76",
        (0x41, Some(0xd0c)) => "ARM Neoverse-N1",
        (0x41, Some(0xd0d)) => "ARM Cortex-A77",
        (0x41, Some(0xd40)) => "ARM Neoverse-V1",
        (0x41, Some(0xd41)) => "ARM Cortex-A78",
        (0x41, Some(0xd49)) => "ARM Neoverse-N2",
        (0x41, Some(0xd4f)) => "ARM Neoverse-V2",
        (0x41, _) => "ARM",
        (0xc0, Some(0xac3)) => "Ampere-1",
        (0xc0, Some(0xac4)) => "Ampere-1a",
        (0xc0, _) => "Ampere",
        _ => return None,
    };
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const X86: &str = "processor\t: 0\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD EPYC 7B13 64-Core Processor\nflags\t\t: fpu vme\n\nprocessor\t: 1\nvendor_id\t: AuthenticAMD\nmodel name\t: AMD EPYC 7B13 64-Core Processor\n";

    #[test]
    fn x86_names_its_model_and_counts_each_processor() {
        assert_eq!(
            parse(X86),
            CpuInfo {
                model: Some("AMD EPYC 7B13 64-Core Processor".into()),
                cores: Some(2),
            }
        );
    }

    #[test]
    fn padded_model_names_are_collapsed() {
        let text =
            "processor\t: 0\nmodel name\t:   Intel(R) Xeon(R)   CPU E5-2680 v4 @ 2.40GHz  \n";
        assert_eq!(
            parse(text).model.as_deref(),
            Some("Intel(R) Xeon(R) CPU E5-2680 v4 @ 2.40GHz")
        );
    }

    /// arm64 without `model name` (e.g. an Ampere Altra VM): named from the
    /// implementer / part codes.
    #[test]
    fn arm64_is_named_from_its_part_code() {
        let text = "processor\t: 0\nBogoMIPS\t: 50.00\nCPU implementer\t: 0x41\nCPU architecture: 8\nCPU part\t: 0xd0c\n\nprocessor\t: 1\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n\nprocessor\t: 2\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n\nprocessor\t: 3\nCPU implementer\t: 0x41\nCPU part\t: 0xd0c\n";
        assert_eq!(
            parse(text),
            CpuInfo {
                model: Some("ARM Neoverse-N1".into()),
                cores: Some(4),
            }
        );
    }

    #[test]
    fn an_unlisted_arm_part_still_names_the_vendor() {
        let text = "processor\t: 0\nCPU implementer\t: 0x41\nCPU part\t: 0xfff\n";
        assert_eq!(parse(text).model.as_deref(), Some("ARM"));
        let ampere = "processor\t: 0\nCPU implementer\t: 0xc0\nCPU part\t: 0xac3\n";
        assert_eq!(parse(ampere).model.as_deref(), Some("Ampere-1"));
    }

    #[test]
    fn an_unknown_vendor_gives_no_model_but_keeps_the_count() {
        let text = "processor\t: 0\nCPU implementer\t: 0x99\nCPU part\t: 0x001\n";
        assert_eq!(
            parse(text),
            CpuInfo {
                model: None,
                cores: Some(1),
            }
        );
    }

    #[test]
    fn empty_input_knows_nothing() {
        assert_eq!(parse(""), CpuInfo::default());
    }

    #[test]
    fn an_oversized_model_is_capped() {
        let text = format!("processor\t: 0\nmodel name\t: {}\n", "x".repeat(500));
        assert_eq!(parse(&text).model.unwrap().len(), MAX_MODEL_LEN);
    }

    #[test]
    fn cpu_info_always_knows_the_cpu_count() {
        // Whatever the machine running the tests is, the count has a fallback.
        assert!(cpu_info().cores.unwrap_or(0) > 0);
    }
}
