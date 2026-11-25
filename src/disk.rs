use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::sh::Result;

const GB: u64 = 1_000_000_000;

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Limit {
    Gb(u64),
    Percent(u64),
}

pub const DEFAULT_TRIM_BELOW: Limit = Limit::Gb(30);
pub const DEFAULT_BUILDS_BELOW: Limit = Limit::Gb(10);

impl Limit {
    pub fn parse(text: &str, key: &str) -> Result<Limit> {
        let trimmed = text.trim();
        let wrong = || format!("{key}: \"{text}\" must be free space as \"30 GB\" or \"15%\"");
        let (number, percent) = match (trimmed.strip_suffix('%'), trimmed.strip_suffix("GB").or_else(|| trimmed.strip_suffix("gb"))) {
            (Some(number), _) => (number, true),
            (None, Some(number)) => (number, false),
            (None, None) => return Err(wrong()),
        };
        let number: u64 = number.trim().parse().map_err(|_| wrong())?;
        match percent {
            true if (1..=90).contains(&number) => Ok(Limit::Percent(number)),
            true => Err(format!("{key}: \"{text}\" must be 1% to 90%")),
            false if number >= 1 => Ok(Limit::Gb(number)),
            false => Err(format!("{key}: \"{text}\" must be at least 1 GB")),
        }
    }

    pub fn bytes(self, total: u64) -> u64 {
        match self {
            Limit::Gb(gb) => gb.saturating_mul(GB),
            Limit::Percent(percent) => total / 100 * percent,
        }
    }

    pub fn shown(self) -> String {
        match self {
            Limit::Gb(gb) => format!("{gb} GB"),
            Limit::Percent(percent) => format!("{percent}%"),
        }
    }
}

// Only two limits of one unit compare without a disk, so a GB limit and a percent pass as given.
pub fn ordered(trim_below: Limit, builds_below: Limit) -> Result<()> {
    let wrong = match (trim_below, builds_below) {
        (Limit::Gb(trim), Limit::Gb(builds)) | (Limit::Percent(trim), Limit::Percent(builds)) => builds >= trim,
        _ => false,
    };
    if wrong {
        return Err(format!("disk.builds_below: {} must be below disk.trim_below, {}, as builds stop only once trimming could not free enough", builds_below.shown(), trim_below.shown()));
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Space {
    pub free: u64,
    pub total: u64,
}

impl Space {
    pub fn below(self, limit: Limit) -> bool {
        self.free < limit.bytes(self.total)
    }
}

// What an unprivileged process may still write, as df's Avail column counts it.
pub fn space(path: &Path) -> Option<Space> {
    use std::os::unix::ffi::OsStrExt;
    let name = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    // SAFETY: statvfs fills the zeroed struct this frame owns from a NUL-terminated path.
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(name.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let unit = stat.f_frsize as u64;
    Some(Space { free: (stat.f_bavail as u64).saturating_mul(unit), total: (stat.f_blocks as u64).saturating_mul(unit) })
}

pub fn gb(bytes: u64) -> String {
    format!("{:.1} GB", bytes as f64 / GB as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_limit_is_free_space_in_gb_or_a_share_of_the_disk() {
        assert_eq!(Limit::parse("30 GB", "disk.trim_below"), Ok(Limit::Gb(30)));
        assert_eq!(Limit::parse("30GB", "disk.trim_below"), Ok(Limit::Gb(30)));
        assert_eq!(Limit::parse("15%", "disk.trim_below"), Ok(Limit::Percent(15)));
        assert!(Limit::parse("30", "disk.trim_below").unwrap_err().contains("\"30 GB\" or \"15%\""));
        assert!(Limit::parse("0 GB", "disk.trim_below").is_err() && Limit::parse("95%", "disk.trim_below").is_err());
        assert_eq!(Limit::Percent(15).bytes(200 * GB), 30 * GB);
        assert_eq!(Limit::Gb(10).bytes(200 * GB), 10 * GB);
    }

    #[test]
    fn a_disk_is_below_a_limit_only_with_less_free_than_it_names() {
        let disk = Space { free: 29 * GB, total: 242 * GB };
        assert!(disk.below(Limit::Gb(30)) && !disk.below(Limit::Gb(29)));
        assert!(disk.below(Limit::Percent(15)) && !disk.below(Limit::Percent(10)));
    }

    #[test]
    fn builds_stop_below_a_lower_limit_than_trimming_starts() {
        assert!(ordered(Limit::Gb(30), Limit::Gb(10)).is_ok());
        assert!(ordered(Limit::Gb(10), Limit::Gb(10)).unwrap_err().contains("must be below disk.trim_below"));
        assert!(ordered(Limit::Percent(5), Limit::Percent(10)).is_err());
        assert!(ordered(Limit::Percent(15), Limit::Gb(10)).is_ok());
    }

    #[test]
    fn the_space_of_a_real_folder_is_read() {
        let space = space(&std::env::temp_dir()).unwrap();
        assert!(space.total > 0 && space.free <= space.total);
    }
}
