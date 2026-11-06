//! Monitors: what each connected screen is, what it can show, and what it's set to.
//!
//! Identity comes from the screen's own EDID (`edid.rs`). The modes it supports and the
//! one in use right now come from the graphics driver (read-only queries on the
//! `/dev/dri` card, which an ordinary logged-in user may do), so this works the same on
//! Wayland and X11. Judging "set lower than it could be" follows what
//! QLinuxDeviceManager (GPL-3.0) shows for monitors.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs::{self, File, OpenOptions};
use std::os::fd::{AsFd, BorrowedFd};
use std::path::Path;

use drm::control::{Device as ControlDevice, Mode, connector};

use crate::descriptions::Descriptions;
use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::{BuiltIn, Bus, Category, Device};
use crate::edid::{self, Edid};

/// One picture mode: size and how many times a second it's redrawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LiveMode {
    pub width: u32,
    pub height: u32,
    pub hertz: f64,
}

impl LiveMode {
    fn pixels(self) -> u32 {
        self.width * self.height
    }

    fn size_text(self) -> String {
        format!("{} × {}", self.width, self.height)
    }
}

/// What the graphics driver says about one connected screen.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Live {
    pub modes: Vec<LiveMode>,
    /// The mode in use right now, if the screen is switched on.
    pub current: Option<LiveMode>,
}

/// Live data by sysfs connector name, e.g. `card1-DP-1`.
pub(crate) type LiveModes = HashMap<String, Live>;

struct Card(File);

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Card {}
impl ControlDevice for Card {}

/// The connector's name as sysfs writes it ("DP", "HDMI-A", ...).
fn interface_name(interface: connector::Interface) -> Option<&'static str> {
    use connector::Interface as I;
    Some(match interface {
        I::DisplayPort => "DP",
        I::HDMIA => "HDMI-A",
        I::HDMIB => "HDMI-B",
        I::EmbeddedDisplayPort => "eDP",
        I::DVID => "DVI-D",
        I::DVII => "DVI-I",
        I::DVIA => "DVI-A",
        I::VGA => "VGA",
        I::LVDS => "LVDS",
        I::Composite => "Composite",
        I::SVideo => "SVIDEO",
        I::Component => "Component",
        I::NinePinDIN => "DIN",
        I::TV => "TV",
        I::DSI => "DSI",
        I::DPI => "DPI",
        _ => return None,
    })
}

fn live_mode(mode: &Mode) -> Option<LiveMode> {
    let (width, height) = mode.size();
    let total = u32::from(mode.hsync().2) * u32::from(mode.vsync().2);
    (total > 0).then(|| LiveMode {
        width: u32::from(width),
        height: u32::from(height),
        hertz: f64::from(mode.clock()) * 1000.0 / f64::from(total),
    })
}

/// Ask every graphics card for its connected screens. Never forces a re-probe (that can
/// make a screen flicker) and never changes anything.
pub(crate) fn read_live() -> LiveModes {
    let mut found = LiveModes::new();
    let Ok(entries) = fs::read_dir("/dev/dri") else {
        return found;
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name
            .strip_prefix("card")
            .is_some_and(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()))
        {
            continue;
        }
        let Ok(file) = OpenOptions::new()
            .read(true)
            .write(true)
            .open(entry.path())
            .or_else(|_| OpenOptions::new().read(true).open(entry.path()))
        else {
            continue;
        };
        let card = Card(file);
        let Ok(resources) = card.resource_handles() else {
            continue;
        };
        for handle in resources.connectors() {
            let Ok(info) = card.get_connector(*handle, false) else {
                continue;
            };
            if info.state() != connector::State::Connected {
                continue;
            }
            let Some(kind) = interface_name(info.interface()) else {
                continue;
            };
            let current = info
                .current_encoder()
                .and_then(|e| card.get_encoder(e).ok())
                .and_then(|e| e.crtc())
                .and_then(|c| card.get_crtc(c).ok())
                .and_then(|c| c.mode())
                .and_then(|m| live_mode(&m));
            found.insert(
                format!("{name}-{kind}-{}", info.interface_id()),
                Live {
                    modes: info.modes().iter().filter_map(live_mode).collect(),
                    current,
                },
            );
        }
    }
    found
}

fn hz_text(hertz: f64) -> String {
    format!("{} Hz", hertz.round() as i64)
}

/// The best picture on offer: the biggest size, then the fastest redraw at that size.
fn best(modes: &[LiveMode]) -> Option<LiveMode> {
    let biggest = modes.iter().map(|m| m.pixels()).max()?;
    modes
        .iter()
        .filter(|m| m.pixels() == biggest)
        .copied()
        .max_by(|a, b| a.hertz.total_cmp(&b.hertz))
}

/// Every size and the refresh rates offered at it, for the Technical level.
fn modes_summary(modes: &[LiveMode]) -> String {
    let mut by_size: BTreeMap<std::cmp::Reverse<(u32, u32)>, BTreeSet<std::cmp::Reverse<i64>>> =
        BTreeMap::new();
    for mode in modes {
        by_size
            .entry(std::cmp::Reverse((mode.width, mode.height)))
            .or_default()
            .insert(std::cmp::Reverse(mode.hertz.round() as i64));
    }
    by_size
        .iter()
        .map(|(std::cmp::Reverse((w, h)), rates)| {
            let rates: Vec<String> = rates
                .iter()
                .map(|std::cmp::Reverse(r)| r.to_string())
                .collect();
            format!("{w} × {h}: {} Hz", rates.join(", "))
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn screen_rows(edid: Option<&Edid>, live: Option<&Live>, source: &str) -> Vec<Detail> {
    let mut rows = Vec::new();
    // Everything the screen can show: the driver's list if we have it, else its own.
    let offered: Vec<LiveMode> = match live {
        Some(live) if !live.modes.is_empty() => live.modes.clone(),
        _ => edid
            .map(|e| {
                e.timings
                    .iter()
                    .map(|t| LiveMode {
                        width: t.width,
                        height: t.height,
                        hertz: t.hertz,
                    })
                    .collect()
            })
            .unwrap_or_default(),
    };
    let best_mode = best(&offered);

    if let Some(current) = live.and_then(|l| l.current) {
        let (verdict, why) = match best_mode {
            Some(top) if top.pixels() > current.pixels() => (
                Verdict::Reduced,
                Some(crate::tr!(
                    "This screen can show {} but is set to a lower size, so the picture may look soft. You can change it in your display settings.",
                    top.size_text()
                )),
            ),
            _ => (Verdict::Good, None),
        };
        rows.push(
            Detail::new(
                crate::tr!("Resolution"),
                current.size_text(),
                source.to_owned(),
            )
            .verdict(verdict, why),
        );
        let fastest_here = offered
            .iter()
            .filter(|m| m.width == current.width && m.height == current.height)
            .map(|m| m.hertz)
            .fold(0.0, f64::max);
        let (verdict, why) = if fastest_here.round() - current.hertz.round() >= 5.0 {
            (
                Verdict::Reduced,
                Some(crate::tr!(
                    "This screen can run at {} at this size but is set to {}. A higher refresh rate makes motion look smoother. You can change it in your display settings.",
                    hz_text(fastest_here),
                    hz_text(current.hertz)
                )),
            )
        } else {
            (Verdict::Good, None)
        };
        rows.push(
            Detail::new(
                crate::tr!("Refresh rate"),
                hz_text(current.hertz),
                source.to_owned(),
            )
            .raw(format!("{:.2} Hz", current.hertz))
            .verdict(verdict, why),
        );
    }
    if let Some(top) = best_mode {
        rows.push(
            Detail::new(
                crate::tr!("Best picture the screen can show"),
                crate::tr!("{} at {}", top.size_text(), hz_text(top.hertz)),
                source.to_owned(),
            )
            .verdict(Verdict::Info, None),
        );
    }
    if let Some(edid) = edid {
        if let Some(inches) = edid.diagonal_inches() {
            rows.push(
                Detail::new(
                    crate::tr!("Size"),
                    crate::tr!("{} inches", format!("{inches:.0}")),
                    source.to_owned(),
                )
                .raw(format!("{} × {} cm", edid.width_cm, edid.height_cm)),
            );
        }
        if let Some(bits) = edid.bits_per_colour {
            rows.push(
                Detail::new(
                    crate::tr!("Colour depth"),
                    crate::tr!("{} bits per colour", bits),
                    source.to_owned(),
                )
                .verdict(
                    Verdict::Info,
                    Some(crate::tr!(
                        "More bits give smoother colour gradients, without visible banding."
                    )),
                ),
            );
        }
        rows.push(
            Detail::new(
                crate::tr!("HDR"),
                if edid.hdr {
                    crate::tr!("Supported")
                } else {
                    crate::tr!("Not reported")
                },
                source.to_owned(),
            )
            .verdict(
                Verdict::Info,
                Some(crate::tr!(
                    "HDR makes very bright and very dark areas look more lifelike. It usually has to be turned on in your display settings too."
                )),
            ),
        );
        if edid.made_week > 0 && edid.made_week < 54 && edid.made_year > 1990 {
            rows.push(Detail::new(
                crate::tr!("Made"),
                crate::tr!("Week {} of {}", edid.made_week, edid.made_year),
                source.to_owned(),
            ));
        }
    }
    rows
}

fn build(
    connector_name: &str,
    dir: &Path,
    edid: Option<&Edid>,
    live: Option<&Live>,
    makers: &HashMap<String, String>,
    descriptions: &Descriptions,
) -> Device {
    let source = dir.join(connector_name).display().to_string();
    // "card1-DP-1" -> card "card1", connector "DP-1".
    let (card, connector) = connector_name
        .split_once('-')
        .unwrap_or(("", connector_name));

    let mut technical = vec![
        Detail::new(
            crate::tr!("Connector"),
            connector.to_owned(),
            source.clone(),
        )
        .technical(),
        Detail::new(crate::tr!("Graphics card"), card.to_owned(), source.clone()).technical(),
    ];
    if let Some(edid) = edid {
        technical.push(
            Detail::new(
                crate::tr!("Screen information standard"),
                format!("EDID {}.{}", edid.version.0, edid.version.1),
                source.clone(),
            )
            .technical(),
        );
        technical.push(
            Detail::new(
                crate::tr!("Maker code"),
                edid.maker_code.clone(),
                source.clone(),
            )
            .technical(),
        );
        technical.push(
            Detail::new(
                crate::tr!("Product code"),
                edid.product_code.to_string(),
                source.clone(),
            )
            .technical(),
        );
    }
    if let Some(live) = live.filter(|l| !l.modes.is_empty()) {
        technical.push(
            Detail::new(
                crate::tr!("Every size and refresh rate on offer"),
                modes_summary(&live.modes),
                crate::tr!("The graphics driver"),
            )
            .technical(),
        );
    }

    let maker = edid.and_then(|e| makers.get(&e.maker_code).cloned());
    let name = edid
        .and_then(|e| e.name.clone())
        .unwrap_or_else(|| crate::tr!("Monitor on {}", connector));
    let mut device = Device::built_in(BuiltIn {
        address: "",
        sysfs_path: dir.join(connector_name),
        display_name: name,
        vendor_name: maker,
        category: Category::Displays,
        hardware_id: edid
            .map(|e| format!("{} {}", e.maker_code, e.product_code))
            .unwrap_or_default(),
        description: descriptions
            .describe(Bus::System, &[], 0, 0, Category::Displays)
            .unwrap_or_default(),
        details: vec![
            DetailGroup {
                title: crate::tr!("Screen"),
                items: screen_rows(edid, live, &source),
            },
            DetailGroup {
                title: crate::tr!("Connection"),
                items: technical,
            },
        ],
    });
    device.address = connector_name.to_owned();
    device
}

/// A device for each connected screen.
pub(crate) fn devices(
    drm_dir: &Path,
    pnp_ids: &Path,
    live: &LiveModes,
    descriptions: &Descriptions,
) -> Vec<Device> {
    let Ok(entries) = fs::read_dir(drm_dir) else {
        return Vec::new();
    };
    let makers = edid::maker_names(pnp_ids);
    let mut names: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("card") && n.contains('-'))
        .filter(|n| {
            fs::read_to_string(drm_dir.join(n).join("status"))
                .is_ok_and(|s| s.trim() == "connected")
        })
        .collect();
    names.sort();
    names
        .iter()
        .map(|name| {
            let edid = fs::read(drm_dir.join(name).join("edid"))
                .ok()
                .and_then(|bytes| edid::parse(&bytes));
            build(
                name,
                drm_dir,
                edid.as_ref(),
                live.get(name),
                &makers,
                descriptions,
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::in_english;

    const MSI: &[u8] = include_bytes!("../tests/fixtures/edid/msi-mp341cq.bin");

    /// A fake `/sys/class/drm` with one connected screen and one empty connector.
    fn drm_dir() -> tempfile::TempDir {
        let temp = tempfile::tempdir().expect("tempdir");
        let root = temp.path();
        fs::create_dir_all(root.join("card1-DP-1")).expect("mkdir");
        fs::write(root.join("card1-DP-1/status"), "connected\n").expect("write");
        fs::write(root.join("card1-DP-1/edid"), MSI).expect("write");
        fs::create_dir_all(root.join("card1-DP-2")).expect("mkdir");
        fs::write(root.join("card1-DP-2/status"), "disconnected\n").expect("write");
        fs::create_dir_all(root.join("card1")).expect("mkdir");
        temp
    }

    fn mode(width: u32, height: u32, hertz: f64) -> LiveMode {
        LiveMode {
            width,
            height,
            hertz,
        }
    }

    fn live(current: LiveMode) -> LiveModes {
        let modes = vec![
            mode(3440, 1440, 59.97),
            mode(3440, 1440, 100.0),
            mode(2560, 1440, 99.95),
            mode(1920, 1080, 60.0),
        ];
        LiveModes::from([(
            "card1-DP-1".to_owned(),
            Live {
                modes,
                current: Some(current),
            },
        )])
    }

    fn build_one(live: &LiveModes) -> Device {
        let temp = drm_dir();
        let mut found = in_english(|| {
            devices(
                temp.path(),
                &temp.path().join("pnp.ids"),
                live,
                &Descriptions::embedded(),
            )
        });
        assert_eq!(found.len(), 1, "only the connected screen is listed");
        found.remove(0)
    }

    fn item<'a>(device: &'a Device, label: &str) -> &'a Detail {
        device
            .details
            .iter()
            .flat_map(|g| &g.items)
            .find(|d| d.label == label)
            .unwrap_or_else(|| panic!("no detail {label:?}"))
    }

    #[test]
    fn a_screen_at_its_best_is_all_good() {
        let d = build_one(&live(mode(3440, 1440, 100.0)));
        assert_eq!(d.display_name, "MSI MP341CQ");
        assert_eq!(d.vendor_name.as_deref(), Some("MSI"));
        assert_eq!(d.hardware_id, "MSI 12466");
        assert_eq!(d.category, Category::Displays);
        assert_eq!(d.address, "card1-DP-1");
        assert_eq!(item(&d, "Resolution").value, "3440 × 1440");
        assert_eq!(item(&d, "Resolution").verdict, Verdict::Good);
        assert_eq!(item(&d, "Refresh rate").value, "100 Hz");
        assert_eq!(item(&d, "Refresh rate").verdict, Verdict::Good);
        assert_eq!(
            item(&d, "Best picture the screen can show").value,
            "3440 × 1440 at 100 Hz"
        );
        assert_eq!(item(&d, "Size").value, "34 inches");
        assert_eq!(item(&d, "Size").raw.as_deref(), Some("80 × 33 cm"));
        assert_eq!(item(&d, "Colour depth").value, "10 bits per colour");
        assert_eq!(item(&d, "HDR").value, "Supported");
        assert_eq!(item(&d, "Made").value, "Week 39 of 2025");
        assert_eq!(item(&d, "Maker code").value, "MSI");
        assert!(
            item(&d, "Every size and refresh rate on offer")
                .value
                .starts_with("3440 × 1440: 100, 60 Hz; 2560 × 1440: 100 Hz")
        );
    }

    #[test]
    fn a_screen_set_to_a_slower_refresh_rate_is_flagged_with_the_numbers() {
        let d = build_one(&live(mode(3440, 1440, 59.97)));
        let rate = item(&d, "Refresh rate");
        assert_eq!(rate.value, "60 Hz");
        assert_eq!(rate.verdict, Verdict::Reduced);
        assert!(
            rate.explanation
                .as_deref()
                .is_some_and(|t| t.contains("run at 100 Hz at this size but is set to 60 Hz"))
        );
        assert_eq!(item(&d, "Resolution").verdict, Verdict::Good);
    }

    #[test]
    fn a_screen_below_its_native_size_is_flagged() {
        let d = build_one(&live(mode(1920, 1080, 60.0)));
        let size = item(&d, "Resolution");
        assert_eq!(size.verdict, Verdict::Reduced);
        assert!(
            size.explanation
                .as_deref()
                .is_some_and(|t| t.contains("can show 3440 × 1440"))
        );
        assert_eq!(
            item(&d, "Refresh rate").verdict,
            Verdict::Good,
            "60 Hz is the fastest it offers at 1920 × 1080"
        );
    }

    #[test]
    fn without_live_data_the_screens_own_information_still_gives_its_best_picture() {
        let d = build_one(&LiveModes::new());
        assert!(
            !d.details
                .iter()
                .flat_map(|g| &g.items)
                .any(|i| i.label == "Resolution" || i.label == "Refresh rate"),
            "no current mode to judge"
        );
        assert_eq!(
            item(&d, "Best picture the screen can show").value,
            "3440 × 1440 at 100 Hz"
        );
        assert_eq!(d.vendor_name.as_deref(), Some("MSI"));
    }

    #[test]
    fn an_unreadable_edid_still_lists_the_screen() {
        let temp = drm_dir();
        fs::write(temp.path().join("card1-DP-1/edid"), b"junk").expect("write");
        let found = in_english(|| {
            devices(
                temp.path(),
                &temp.path().join("pnp.ids"),
                &LiveModes::new(),
                &Descriptions::embedded(),
            )
        });
        assert_eq!(found[0].display_name, "Monitor on DP-1");
        assert_eq!(found[0].hardware_id, "");
    }

    #[test]
    fn serial_numbers_never_appear() {
        let d = build_one(&live(mode(3440, 1440, 100.0)));
        let shown = format!("{:?}", d.details).to_lowercase();
        assert!(!shown.contains("serial"));
    }

    #[test]
    fn no_drm_folder_means_no_screens() {
        let found = devices(
            Path::new("/nonexistent/drm"),
            Path::new("/nonexistent/pnp"),
            &LiveModes::new(),
            &Descriptions::embedded(),
        );
        assert!(found.is_empty());
    }
}
