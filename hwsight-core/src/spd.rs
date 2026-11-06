//! DDR5 SPD: the small chip on every memory stick that stores what the stick can do.
//!
//! The kernel's `spd5118` driver exposes each chip as a world-readable `eeprom` file, so
//! this needs no admin rights. Layout (JEDEC JESD400-5, plus Intel's XMP 3.0 block and
//! AMD's EXPO block) was checked against real sticks: the basic speed decodes to 4800 and
//! the XMP profiles to 6400, 6000, and 5600.

use std::fs;
use std::path::Path;

/// What one stick says about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spd {
    pub part_number: String,
    /// The standard (JEDEC) speed every DDR5 stick can run at, in MT/s (e.g. 4800).
    pub base_mts: Option<u32>,
    /// Speeds of the factory overclock profiles (Intel XMP 3.0), fastest first.
    pub xmp_mts: Vec<u32>,
    /// The stick carries an AMD EXPO profile.
    pub has_expo: bool,
    /// Speeds of the EXPO profiles (up to two), fastest first.
    pub expo_mts: Vec<u32>,
}

impl Spd {
    /// The fastest speed this stick says it can run at, profiles included.
    pub fn best_mts(&self) -> Option<u32> {
        self.xmp_mts
            .iter()
            .chain(&self.expo_mts)
            .max()
            .copied()
            .or(self.base_mts)
    }
}

fn le16(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes([
        *bytes.get(offset)?,
        *bytes.get(offset + 1)?,
    ]))
}

/// Transfers per second (MT/s) from the shortest clock cycle in picoseconds, rounded to
/// the nearest 100 (the way speeds are sold). Cycles outside DDR5's range are rejected.
fn mts_from_cycle(picoseconds: u16) -> Option<u32> {
    if !(240..=450).contains(&picoseconds) {
        return None;
    }
    let exact = 2_000_000.0 / f64::from(picoseconds);
    Some(((exact / 100.0).round() as u32) * 100)
}

const DRAM_TYPE_DDR5: u8 = 0x12;
const XMP_HEADER: usize = 0x280;
const XMP_PROFILES: [usize; 5] = [0x2C0, 0x300, 0x340, 0x380, 0x3C0];
const EXPO_HEADER: usize = 0x340;
/// The EXPO block is a 10-byte header followed by up to two 40-byte profiles; each
/// profile's shortest cycle time sits 4 bytes in. (Read from real sticks: 333 and 357 ps,
/// which are the 6000 and 5600 their XMP profiles also list.)
const EXPO_PROFILES: [usize; 2] = [0x34A, 0x372];

/// Decode a DDR5 SPD dump. `None` when it isn't DDR5 or is too short to be one.
pub fn parse_ddr5(bytes: &[u8]) -> Option<Spd> {
    if bytes.len() < 1024 || bytes[2] != DRAM_TYPE_DDR5 {
        return None;
    }
    let part_number = bytes[521..551]
        .iter()
        .take_while(|b| **b != 0)
        .map(|b| char::from(*b))
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect::<String>()
        .trim()
        .to_owned();
    let base_mts = le16(bytes, 20).and_then(mts_from_cycle);

    let mut xmp_mts = Vec::new();
    if bytes[XMP_HEADER..XMP_HEADER + 2] == [0x0C, 0x4A] {
        for start in XMP_PROFILES {
            // The shortest cycle time sits 5 bytes into each 64-byte profile.
            if let Some(mts) = le16(bytes, start + 5).and_then(mts_from_cycle)
                && !xmp_mts.contains(&mts)
            {
                xmp_mts.push(mts);
            }
        }
        xmp_mts.sort_unstable_by(|a, b| b.cmp(a));
    }
    let has_expo = bytes[EXPO_HEADER..EXPO_HEADER + 4] == *b"EXPO";
    let mut expo_mts = Vec::new();
    if has_expo {
        for start in EXPO_PROFILES {
            if let Some(mts) = le16(bytes, start + 4).and_then(mts_from_cycle)
                && !expo_mts.contains(&mts)
            {
                expo_mts.push(mts);
            }
        }
        expo_mts.sort_unstable_by(|a, b| b.cmp(a));
    }
    Some(Spd {
        part_number,
        base_mts,
        xmp_mts,
        has_expo,
        expo_mts,
    })
}

/// Every DDR5 stick whose SPD chip the kernel exposes under `dir` (normally
/// `/sys/bus/i2c/drivers/spd5118`), in the order of their I2C addresses.
pub fn read_all(dir: &Path) -> Vec<Spd> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut chips: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|e| e.path().join("eeprom"))
        .filter(|p| p.is_file())
        .collect();
    chips.sort();
    chips
        .iter()
        .filter_map(|p| fs::read(p).ok())
        .filter_map(|bytes| parse_ddr5(&bytes))
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A fake SPD dump: part number, standard cycle time, and XMP profile cycle times.
    pub(crate) fn fake_spd(part: &str, base_cycle: u16, xmp_cycles: &[u16], expo: bool) -> Vec<u8> {
        let mut b = vec![0u8; 1024];
        b[2] = DRAM_TYPE_DDR5;
        b[20..22].copy_from_slice(&base_cycle.to_le_bytes());
        b[521..521 + part.len()].copy_from_slice(part.as_bytes());
        for byte in &mut b[521 + part.len()..551] {
            *byte = b' ';
        }
        if !xmp_cycles.is_empty() {
            b[XMP_HEADER..XMP_HEADER + 2].copy_from_slice(&[0x0C, 0x4A]);
        }
        for (cycle, start) in xmp_cycles.iter().zip(XMP_PROFILES) {
            b[start + 5..start + 7].copy_from_slice(&cycle.to_le_bytes());
        }
        if expo {
            b[EXPO_HEADER..EXPO_HEADER + 4].copy_from_slice(b"EXPO");
            for (cycle, start) in [333u16, 357].iter().zip(EXPO_PROFILES) {
                b[start + 4..start + 6].copy_from_slice(&cycle.to_le_bytes());
            }
        }
        b
    }

    #[test]
    fn decodes_the_speeds_of_real_sticks() {
        // 416 ps is 4800, and 312, 333, 357 ps are 6400, 6000, 5600: the values read from
        // the Team Group sticks this was written against.
        let spd = parse_ddr5(&fake_spd("UD5-6400", 416, &[312, 333], false)).expect("ddr5");
        assert_eq!(spd.part_number, "UD5-6400");
        assert_eq!(spd.base_mts, Some(4800));
        assert_eq!(spd.xmp_mts, [6400, 6000]);
        assert_eq!(spd.best_mts(), Some(6400));
        assert!(!spd.has_expo);
        let slower = parse_ddr5(&fake_spd("UD5-6000", 416, &[333, 357], true)).expect("ddr5");
        assert_eq!(slower.xmp_mts, [6000, 5600]);
        assert!(slower.has_expo);
        assert_eq!(slower.expo_mts, [6000, 5600]);
    }

    #[test]
    fn decodes_an_expo_block_dumped_from_a_real_stick() {
        // The first 0x40 bytes of the block, as read from a Team Group DDR5-6000 stick.
        let block: [u8; 0x40] = [
            0x45, 0x58, 0x50, 0x4f, 0x10, 0x33, 0x11, 0x00, 0x00, 0x00, 0x25, 0x25, 0x30, 0x00,
            0x4d, 0x01, 0x6e, 0x31, 0x6e, 0x31, 0x6e, 0x31, 0x76, 0x65, 0xe4, 0x96, 0x30, 0x75,
            0x27, 0x01, 0xa0, 0x00, 0x82, 0x00, 0x88, 0x13, 0x88, 0x13, 0x20, 0x4e, 0x10, 0x27,
            0xaa, 0x29, 0x10, 0x27, 0xc4, 0x09, 0x4c, 0x1d, 0x24, 0x24, 0x30, 0x00, 0x65, 0x01,
            0xc8, 0x37, 0xc8, 0x37, 0xc8, 0x37, 0x24, 0x75,
        ];
        let mut bytes = fake_spd("UD5-6000", 416, &[], false);
        bytes[EXPO_HEADER..EXPO_HEADER + 0x40].copy_from_slice(&block);
        let spd = parse_ddr5(&bytes).expect("ddr5");
        assert_eq!(spd.expo_mts, [6000, 5600]);
        assert_eq!(spd.best_mts(), Some(6000));
    }

    #[test]
    fn profiles_are_sorted_fastest_first_and_deduplicated() {
        let spd = parse_ddr5(&fake_spd("X", 416, &[357, 312, 357], false)).expect("ddr5");
        assert_eq!(spd.xmp_mts, [6400, 5600]);
    }

    #[test]
    fn a_stick_without_profiles_uses_its_standard_speed() {
        let spd = parse_ddr5(&fake_spd("PLAIN", 416, &[], false)).expect("ddr5");
        assert!(spd.xmp_mts.is_empty());
        assert_eq!(spd.best_mts(), Some(4800));
    }

    #[test]
    fn rejects_other_memory_types_short_dumps_and_nonsense_cycles() {
        let mut ddr4 = fake_spd("X", 416, &[], false);
        ddr4[2] = 0x0C;
        assert_eq!(parse_ddr5(&ddr4), None);
        assert_eq!(parse_ddr5(&[0x12; 100]), None);
        let spd = parse_ddr5(&fake_spd("X", 0, &[65535, 100], false)).expect("ddr5");
        assert_eq!(spd.base_mts, None);
        assert!(spd.xmp_mts.is_empty(), "impossible cycle times are ignored");
        assert_eq!(mts_from_cycle(384), Some(5200));
        assert_eq!(mts_from_cycle(278), Some(7200));
    }

    #[test]
    fn reads_every_chip_in_a_folder() {
        let temp = tempfile::tempdir().expect("tempdir");
        for (name, part) in [("10-0052", "B"), ("10-0051", "A")] {
            let dir = temp.path().join(name);
            fs::create_dir_all(&dir).expect("mkdir");
            fs::write(dir.join("eeprom"), fake_spd(part, 416, &[333], false)).expect("write");
        }
        fs::write(temp.path().join("bind"), "").expect("write");
        let parts: Vec<_> = read_all(temp.path())
            .into_iter()
            .map(|s| s.part_number)
            .collect();
        assert_eq!(parts, ["A", "B"]);
        assert!(read_all(&temp.path().join("missing")).is_empty());
    }
}
