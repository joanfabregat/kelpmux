//! Disk usage for every real block-device filesystem.
//!
//! Filesystems are listed from `/proc/self/mounts`, in mount order, so the
//! root filesystem usually comes first and positions stay stable. Selection:
//!
//! - only sources under `/dev/`, which drops tmpfs, proc, overlay and friends;
//! - pseudo or read-only image types (squashfs, …) skipped even when backed
//!   by a device, so snap and loop images do not show up;
//! - one entry per source device, so bind mounts do not count twice;
//! - filesystems smaller than a minimum size skipped, which removes small
//!   boot and EFI partitions.
//!
//! The percentage matches `df`: used / (used + available to unprivileged
//! users), rounded up.

use std::ffi::CString;
use std::mem::MaybeUninit;

const SKIPPED_TYPES: &[&str] = &["tmpfs", "devtmpfs", "efivarfs", "squashfs", "overlay"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub source: String,
    pub target: String,
}

/// Decodes the octal escapes (`\040` for a space, …) the kernel uses in
/// mount table fields.
fn unescape(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\'
            && i + 3 < bytes.len()
            && bytes[i + 1..i + 4]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            let value = (bytes[i + 1] - b'0') as u32 * 64
                + (bytes[i + 2] - b'0') as u32 * 8
                + (bytes[i + 3] - b'0') as u32;
            if let Ok(byte) = u8::try_from(value) {
                out.push(byte);
                i += 4;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Real, de-duplicated device mounts, in mount-table order.
pub fn parse_mounts(mounts: &str) -> Vec<Mount> {
    let mut seen = Vec::new();
    let mut out = Vec::new();
    for line in mounts.lines() {
        let mut fields = line.split_whitespace();
        let (Some(source), Some(target), Some(fstype)) =
            (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };
        if !source.starts_with("/dev/") || SKIPPED_TYPES.contains(&fstype) {
            continue;
        }
        if seen.iter().any(|s| s == source) {
            continue;
        }
        seen.push(source.to_string());
        out.push(Mount {
            source: unescape(source),
            target: unescape(target),
        });
    }
    out
}

/// Filesystem sizes, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Space {
    pub size: u64,
    pub free: u64,
    pub available: u64,
}

/// `df`'s percentage: used over used + available, rounded up. `None` for a
/// filesystem that reports no usable blocks.
pub fn df_percent(space: Space) -> Option<u8> {
    let used = space.size.saturating_sub(space.free);
    let denominator = u128::from(used) + u128::from(space.available);
    if denominator == 0 {
        return None;
    }
    Some(((used as u128 * 100).div_ceil(denominator)).min(100) as u8)
}

pub fn statvfs(path: &str) -> Option<Space> {
    let path = CString::new(path).ok()?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    // SAFETY: `path` is a valid NUL-terminated string and `stat` points to
    // writable memory of the size statvfs expects.
    let rc = unsafe { libc::statvfs(path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return None;
    }
    // SAFETY: statvfs returned 0, so it filled the struct.
    let stat = unsafe { stat.assume_init() };
    let fragment: u64 = stat.f_frsize;
    Some(Space {
        size: stat.f_blocks.checked_mul(fragment)?,
        free: stat.f_bfree.checked_mul(fragment)?,
        available: stat.f_bavail.checked_mul(fragment)?,
    })
}

/// Usage percentages for every selected filesystem of at least `min_bytes`.
pub fn read(min_bytes: u64) -> Vec<u8> {
    let Ok(mounts) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    readings(&mounts, min_bytes)
}

pub fn readings(mounts: &str, min_bytes: u64) -> Vec<u8> {
    readings_with_mounts(mounts, min_bytes)
        .into_iter()
        .map(|(_, percent)| percent)
        .collect()
}
pub fn readings_with_mounts(mounts: &str, min_bytes: u64) -> Vec<(String, u8)> {
    parse_mounts(mounts)
        .into_iter()
        .filter_map(|mount| {
            let space = statvfs(&mount.target).filter(|space| space.size >= min_bytes)?;
            Some((mount.target, df_percent(space)?))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOUNTS: &str = "\
sysfs /sys sysfs rw,nosuid 0 0
proc /proc proc rw 0 0
/dev/sda1 / ext4 rw,relatime 0 0
tmpfs /run tmpfs rw 0 0
/dev/sda15 /boot/efi vfat rw 0 0
/dev/loop0 /snap/core/1 squashfs ro 0 0
/dev/sdb /mnt/my\\040data ext4 rw 0 0
/dev/sda1 /var/lib/bind ext4 rw 0 0
overlay /merged overlay rw 0 0
";

    #[test]
    fn selects_real_unique_devices_in_order() {
        let mounts = parse_mounts(MOUNTS);
        let targets: Vec<_> = mounts.iter().map(|m| m.target.as_str()).collect();
        assert_eq!(targets, ["/", "/boot/efi", "/mnt/my data"]);
    }

    #[test]
    fn unescapes_octal_sequences() {
        assert_eq!(unescape("a\\040b\\011c"), "a b\tc");
        assert_eq!(unescape("trailing\\04"), "trailing\\04");
        assert_eq!(unescape("plain"), "plain");
    }

    #[test]
    fn percent_matches_df_rounding() {
        // used 61, available 39: 61% exactly.
        let s = Space {
            size: 105,
            free: 44,
            available: 39,
        };
        assert_eq!(df_percent(s), Some(61));
        // used 1, available 199: 0.5% rounds up to 1%.
        let s = Space {
            size: 200,
            free: 199,
            available: 199,
        };
        assert_eq!(df_percent(s), Some(1));
        let empty = Space {
            size: 0,
            free: 0,
            available: 0,
        };
        assert_eq!(df_percent(empty), None);
    }

    #[test]
    fn statvfs_reads_root() {
        let space = statvfs("/").expect("statvfs(/)");
        assert!(space.size > 0);
        assert!(space.free <= space.size);
    }
}
