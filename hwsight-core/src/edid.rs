//! A monitor's EDID: the small block of facts every screen reports about itself.
//!
//! Only what's useful and safe to show is decoded. The serial number is deliberately
//! ignored (it identifies one particular screen). Layout: VESA EDID 1.4 base block, plus
//! the CTA-861 extension for HDR support. The monitor-identity fields and the HDR check
//! follow what QLinuxDeviceManager (GPL-3.0) reads.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

/// A timing the screen lists for itself.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Timing {
    pub width: u32,
    pub height: u32,
    pub hertz: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Edid {
    /// The three-letter maker code, e.g. "MSI".
    pub maker_code: String,
    pub product_code: u16,
    pub made_week: u8,
    pub made_year: u16,
    pub version: (u8, u8),
    pub digital: bool,
    pub bits_per_colour: Option<u8>,
    pub width_cm: u8,
    pub height_cm: u8,
    /// The screen's own product name, e.g. "MSI MP341CQ".
    pub name: Option<String>,
    pub hdr: bool,
    /// The first (preferred) detailed timing.
    pub preferred: Option<Timing>,
    /// Every detailed timing in the base block.
    pub timings: Vec<Timing>,
}

impl Edid {
    /// The diagonal in inches, when the screen reports a physical size.
    pub fn diagonal_inches(&self) -> Option<f64> {
        (self.width_cm > 0 && self.height_cm > 0).then(|| {
            (f64::from(self.width_cm).powi(2) + f64::from(self.height_cm).powi(2)).sqrt() / 2.54
        })
    }
}

fn maker_code(bytes: [u8; 2]) -> String {
    let word = u16::from_be_bytes(bytes);
    [10, 5, 0]
        .iter()
        .map(|shift| {
            let letter = ((word >> shift) & 0x1F) as u8;
            char::from(letter + 64)
        })
        .filter(char::is_ascii_uppercase)
        .collect()
}

fn text(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|b| *b == 0x0A).unwrap_or(bytes.len());
    let text: String = bytes[..end]
        .iter()
        .map(|b| char::from(*b))
        .filter(|c| c.is_ascii_graphic() || *c == ' ')
        .collect();
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn timing(descriptor: &[u8]) -> Option<Timing> {
    let clock_khz = u32::from(u16::from_le_bytes([descriptor[0], descriptor[1]])) * 10;
    let width = u32::from(descriptor[2]) | (u32::from(descriptor[4] & 0xF0) << 4);
    let width_blank = u32::from(descriptor[3]) | (u32::from(descriptor[4] & 0x0F) << 8);
    let height = u32::from(descriptor[5]) | (u32::from(descriptor[7] & 0xF0) << 4);
    let height_blank = u32::from(descriptor[6]) | (u32::from(descriptor[7] & 0x0F) << 8);
    let total = (width + width_blank) * (height + height_blank);
    (clock_khz > 0 && width > 0 && height > 0 && total > 0).then(|| Timing {
        width,
        height,
        hertz: f64::from(clock_khz) * 1000.0 / f64::from(total),
    })
}

/// Whether a CTA-861 extension block lists an HDR static metadata data block.
fn cta_has_hdr(block: &[u8]) -> bool {
    if block.len() < 128 || block[0] != 0x02 {
        return false;
    }
    let end = usize::from(block[2]).clamp(4, 127);
    let mut at = 4;
    while at < end {
        let tag = block[at] >> 5;
        let length = usize::from(block[at] & 0x1F);
        // Tag 7 is an extended block; its first byte says which. 6 is HDR static metadata.
        if tag == 7 && length >= 1 && block.get(at + 1) == Some(&6) {
            return true;
        }
        at += 1 + length;
    }
    false
}

/// Decode an EDID. `None` when it isn't a valid base block.
pub fn parse(bytes: &[u8]) -> Option<Edid> {
    if bytes.len() < 128 || bytes[..8] != [0, 255, 255, 255, 255, 255, 255, 0] {
        return None;
    }
    if bytes[..128].iter().map(|b| u32::from(*b)).sum::<u32>() % 256 != 0 {
        return None;
    }
    let input = bytes[20];
    let digital = input & 0x80 != 0;
    let bits_per_colour = if digital {
        match (input >> 4) & 0x07 {
            1 => Some(6),
            2 => Some(8),
            3 => Some(10),
            4 => Some(12),
            5 => Some(14),
            6 => Some(16),
            _ => None,
        }
    } else {
        None
    };

    let mut name = None;
    let mut timings = Vec::new();
    for descriptor in bytes[54..126].as_chunks::<18>().0 {
        if descriptor[0] == 0 && descriptor[1] == 0 {
            if descriptor[3] == 0xFC {
                name = text(&descriptor[5..18]);
            }
        } else if let Some(t) = timing(descriptor) {
            timings.push(t);
        }
    }
    let extensions = usize::from(bytes[126]);
    let hdr = (1..=extensions)
        .filter_map(|n| bytes.get(n * 128..(n + 1) * 128))
        .any(cta_has_hdr);

    Some(Edid {
        maker_code: maker_code([bytes[8], bytes[9]]),
        product_code: u16::from_le_bytes([bytes[10], bytes[11]]),
        made_week: bytes[16],
        made_year: 1990 + u16::from(bytes[17]),
        version: (bytes[18], bytes[19]),
        digital,
        bits_per_colour,
        width_cm: bytes[21],
        height_cm: bytes[22],
        name,
        hdr,
        preferred: timings.first().copied(),
        timings,
    })
}

/// Maker codes to names, from the system's `pnp.ids` (hwdata) when it has it.
pub fn maker_names(pnp_ids: &Path) -> HashMap<String, String> {
    let mut names: HashMap<String, String> = [
        ("ACR", "Acer"),
        ("AOC", "AOC"),
        ("AUS", "ASUS"),
        ("BNQ", "BenQ"),
        ("DEL", "Dell"),
        ("GSM", "LG"),
        ("HWP", "HP"),
        ("LEN", "Lenovo"),
        ("MSI", "MSI"),
        ("PHL", "Philips"),
        ("SAM", "Samsung"),
        ("SNY", "Sony"),
        ("VSC", "ViewSonic"),
    ]
    .into_iter()
    .map(|(code, name)| (code.to_owned(), name.to_owned()))
    .collect();
    if let Ok(text) = fs::read_to_string(pnp_ids) {
        for line in text.lines() {
            if let Some((code, name)) = line.split_once('\t')
                && code.len() == 3
                && !name.trim().is_empty()
            {
                // Keep the short names above for the common makers; fill in the rest.
                names
                    .entry(code.to_owned())
                    .or_insert_with(|| name.trim().to_owned());
            }
        }
    }
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real monitor's EDID with its serial number removed.
    const MSI: &[u8] = include_bytes!("../tests/fixtures/edid/msi-mp341cq.bin");

    #[test]
    fn decodes_a_real_monitor() {
        let edid = parse(MSI).expect("valid");
        assert_eq!(edid.maker_code, "MSI");
        assert_eq!(edid.product_code, 12466);
        assert_eq!(edid.name.as_deref(), Some("MSI MP341CQ"));
        assert_eq!((edid.made_week, edid.made_year), (39, 2025));
        assert_eq!(edid.version, (1, 4));
        assert!(edid.digital);
        assert_eq!(edid.bits_per_colour, Some(10));
        assert_eq!((edid.width_cm, edid.height_cm), (80, 33));
        assert!(
            edid.hdr,
            "the HDR static metadata block is in the CTA extension"
        );
        let inches = edid.diagonal_inches().expect("size");
        assert!((inches - 34.1).abs() < 0.1, "{inches}");
        let preferred = edid.preferred.expect("preferred");
        assert_eq!((preferred.width, preferred.height), (3440, 1440));
        assert!((preferred.hertz - 59.97).abs() < 0.01);
        let fastest = edid
            .timings
            .iter()
            .filter(|t| t.width == 3440)
            .map(|t| t.hertz)
            .fold(0.0, f64::max);
        assert!(
            (fastest - 100.0).abs() < 0.01,
            "the 100 Hz timing is listed too"
        );
    }

    #[test]
    fn the_serial_number_is_not_in_the_fixture_or_the_decoded_data() {
        assert_eq!(&MSI[12..16], &[0, 0, 0, 0]);
        let shown = format!("{:?}", parse(MSI).expect("valid"));
        assert!(!shown.to_lowercase().contains("serial"));
    }

    #[test]
    fn rejects_garbage_and_corrupt_blocks() {
        assert_eq!(parse(&[0; 128]), None);
        assert_eq!(parse(&MSI[..100]), None);
        let mut corrupt = MSI.to_vec();
        corrupt[30] ^= 0xFF;
        assert_eq!(parse(&corrupt), None, "a failed checksum is not trusted");
    }

    #[test]
    fn a_screen_without_extensions_has_no_hdr() {
        let mut base = MSI[..128].to_vec();
        base[126] = 0;
        base[127] = (256 - base[..127].iter().map(|b| u32::from(*b)).sum::<u32>() % 256) as u8;
        let edid = parse(&base).expect("valid");
        assert!(!edid.hdr);
    }

    #[test]
    fn maker_names_come_from_pnp_ids_with_a_built_in_fallback() {
        let temp = tempfile::tempdir().expect("tempdir");
        let file = temp.path().join("pnp.ids");
        fs::write(&file, "ZZZ\tZed Displays Ltd\nMSI\tMicrostep\n").expect("write");
        let names = maker_names(&file);
        assert_eq!(
            names.get("ZZZ").map(String::as_str),
            Some("Zed Displays Ltd")
        );
        assert_eq!(
            names.get("MSI").map(String::as_str),
            Some("MSI"),
            "common makers keep short names"
        );
        let fallback = maker_names(&temp.path().join("missing"));
        assert_eq!(fallback.get("DEL").map(String::as_str), Some("Dell"));
        assert_eq!(fallback.get("ZZZ"), None);
    }
}
