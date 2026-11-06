//! Details for the "More details" and "Technical" levels of the details panel.
//!
//! Every value says where it came from (so an expert can check it and we can debug a
//! wrong reading), carries a plain-English verdict when it can be judged, and is marked
//! as technical when a beginner would not miss it. The Simple level shows none of this.

use serde::Serialize;

/// How good a value is, judged with the same rules as the diagnosis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
    /// Nothing to judge (a version number, a name).
    Info,
    Good,
    /// Works, but slower or weaker than it could be.
    Reduced,
    Problem,
}

impl Verdict {
    /// The kind name the GUI uses for colours (same words as the status pills).
    pub fn kind(self) -> &'static str {
        match self {
            Self::Info => "neutral",
            Self::Good => "ok",
            Self::Reduced => "warn",
            Self::Problem => "error",
        }
    }
}

/// One fact about a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Detail {
    /// Plain-English name, e.g. "Link speed".
    pub label: String,
    /// The value with units, in plain words, e.g. "PCIe 5.0 (32 GT/s)".
    pub value: String,
    /// The value exactly as read, when it differs from `value`.
    pub raw: Option<String>,
    pub verdict: Verdict,
    /// Plain English about what this means, shown as a tooltip or under the value.
    pub explanation: Option<String>,
    /// Where the value came from: a file path or a named source.
    pub source: String,
    /// Only shown in the Technical level (the "More details" level skips it).
    pub technical: bool,
}

impl Detail {
    pub fn new(label: String, value: String, source: String) -> Self {
        Self {
            label,
            value,
            raw: None,
            verdict: Verdict::Info,
            explanation: None,
            source,
            technical: false,
        }
    }

    pub fn raw(mut self, raw: impl Into<String>) -> Self {
        self.raw = Some(raw.into());
        self
    }

    pub fn verdict(mut self, verdict: Verdict, explanation: Option<String>) -> Self {
        self.verdict = verdict;
        self.explanation = explanation;
        self
    }

    pub fn technical(mut self) -> Self {
        self.technical = true;
        self
    }
}

/// Details that belong together, e.g. "PCIe connection".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DetailGroup {
    pub title: String,
    pub items: Vec<Detail>,
}

/// Which details to show. The levels build on each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Simple,
    More,
    Technical,
}

impl Level {
    /// From the number stored in the settings file or used by the GUI (0, 1, 2).
    pub fn from_index(index: i32) -> Self {
        match index {
            i32::MIN..=0 => Self::Simple,
            1 => Self::More,
            _ => Self::Technical,
        }
    }

    pub fn index(self) -> i32 {
        self as i32
    }

    /// Whether a detail is shown at this level.
    pub fn shows(self, detail: &Detail) -> bool {
        match self {
            Self::Simple => false,
            Self::More => !detail.technical,
            Self::Technical => true,
        }
    }
}

/// The groups of `groups` that have something to show at `level`, with only the details
/// that belong to it.
pub fn for_level(groups: &[DetailGroup], level: Level) -> Vec<DetailGroup> {
    groups
        .iter()
        .filter_map(|g| {
            let items: Vec<Detail> = g.items.iter().filter(|d| level.shows(d)).cloned().collect();
            (!items.is_empty()).then(|| DetailGroup {
                title: g.title.clone(),
                items,
            })
        })
        .collect()
}

/// A size in bytes as plain text with binary units, e.g. "256 MiB".
pub fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["bytes", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes;
    let mut unit = 0;
    while value >= 1024 && value.is_multiple_of(1024) && unit < UNITS.len() - 1 {
        value /= 1024;
        unit += 1;
    }
    format!("{value} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detail(technical: bool) -> Detail {
        let d = Detail::new("Label".into(), "Value".into(), "somewhere".into());
        if technical { d.technical() } else { d }
    }

    #[test]
    fn levels_show_the_right_details() {
        let groups = vec![
            DetailGroup {
                title: "Both".into(),
                items: vec![detail(false), detail(true)],
            },
            DetailGroup {
                title: "Only technical".into(),
                items: vec![detail(true)],
            },
        ];
        assert!(for_level(&groups, Level::Simple).is_empty());
        let more = for_level(&groups, Level::More);
        assert_eq!(more.len(), 1);
        assert_eq!(more[0].items.len(), 1);
        let all = for_level(&groups, Level::Technical);
        assert_eq!(all.len(), 2);
        assert_eq!(all[0].items.len(), 2);
    }

    #[test]
    fn level_numbers_round_trip_and_clamp() {
        for level in [Level::Simple, Level::More, Level::Technical] {
            assert_eq!(Level::from_index(level.index()), level);
        }
        assert_eq!(Level::from_index(-5), Level::Simple);
        assert_eq!(Level::from_index(99), Level::Technical);
    }

    #[test]
    fn sizes_use_the_largest_exact_unit() {
        assert_eq!(format_bytes(0), "0 bytes");
        assert_eq!(format_bytes(4096), "4 KiB");
        assert_eq!(format_bytes(256 * 1024 * 1024), "256 MiB");
        assert_eq!(format_bytes(16 * 1024 * 1024 * 1024), "16 GiB");
        assert_eq!(format_bytes(1500), "1500 bytes");
    }

    #[test]
    fn verdict_kinds_match_the_status_pill_kinds() {
        assert_eq!(Verdict::Good.kind(), "ok");
        assert_eq!(Verdict::Reduced.kind(), "warn");
        assert_eq!(Verdict::Problem.kind(), "error");
        assert_eq!(Verdict::Info.kind(), "neutral");
    }
}
