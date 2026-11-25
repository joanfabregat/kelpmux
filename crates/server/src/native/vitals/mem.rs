//! Memory and swap usage from `/proc/meminfo`.
//!
//! Memory in use is MemTotal - MemAvailable, the same figure procps `free`
//! reports as "used" since procps-ng 4: page cache the kernel can reclaim
//! does not count. Swap in use is SwapTotal - SwapFree.

use std::fs;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Usage {
    pub memory: u8,
    pub swap: u8,
}

/// Rounded percentage; an empty total (no swap configured) reads 0%.
pub fn rounded_percent(used: u64, total: u64) -> u8 {
    if total == 0 {
        return 0;
    }
    ((u128::from(used.min(total)) * 100 + u128::from(total) / 2) / u128::from(total)) as u8
}

pub fn parse_meminfo(meminfo: &str) -> Option<Usage> {
    let field = |name: &str| -> Option<u64> {
        meminfo.lines().find_map(|line| {
            let rest = line.strip_prefix(name)?.strip_prefix(':')?;
            rest.split_whitespace().next()?.parse().ok()
        })
    };
    let mem_total = field("MemTotal")?;
    let mem_available = field("MemAvailable")?;
    let swap_total = field("SwapTotal").unwrap_or(0);
    let swap_free = field("SwapFree").unwrap_or(0);
    Some(Usage {
        memory: rounded_percent(mem_total.saturating_sub(mem_available), mem_total),
        swap: rounded_percent(swap_total.saturating_sub(swap_free), swap_total),
    })
}

pub fn read() -> Option<Usage> {
    parse_meminfo(&fs::read_to_string("/proc/meminfo").ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_memory_and_swap() {
        let meminfo = "MemTotal:       16000000 kB\n\
                       MemFree:          200000 kB\n\
                       MemAvailable:   12000000 kB\n\
                       SwapCached:            0 kB\n\
                       SwapTotal:       8000000 kB\n\
                       SwapFree:        6000000 kB\n";
        assert_eq!(
            parse_meminfo(meminfo),
            Some(Usage {
                memory: 25,
                swap: 25
            })
        );
    }

    #[test]
    fn missing_swap_reads_zero() {
        let meminfo = "MemTotal: 1000 kB\nMemAvailable: 500 kB\n";
        assert_eq!(parse_meminfo(meminfo).unwrap().swap, 0);
    }

    #[test]
    fn does_not_confuse_prefixed_fields() {
        // SwapCached must not be read as SwapTotal.
        let meminfo = "MemTotal: 1000 kB\nMemAvailable: 500 kB\nSwapCached: 7 kB\n";
        assert_eq!(parse_meminfo(meminfo).unwrap().swap, 0);
    }

    #[test]
    fn requires_memory_fields() {
        assert_eq!(parse_meminfo("SwapTotal: 10 kB\n"), None);
    }

    #[test]
    fn rounds_half_up() {
        assert_eq!(rounded_percent(1, 200), 1);
        assert_eq!(rounded_percent(0, 0), 0);
        assert_eq!(rounded_percent(10, 5), 100);
    }
}
