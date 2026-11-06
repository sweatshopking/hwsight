//! Temperatures and fan speeds from the kernel's sensors, shown on the device each one
//! belongs to (a graphics chip, a Wi-Fi card, a liquid cooler). The processor, drives, and
//! memory sticks have their own readers, so those sensors are skipped here.

use std::fs;
use std::path::Path;

use crate::details::{Detail, DetailGroup, Verdict};
use crate::device::Device;

/// Sensors whose devices are covered elsewhere.
const SKIPPED: [&str; 5] = ["k10temp", "coretemp", "zenpower", "nvme", "spd5118"];

fn read(path: &Path) -> Option<String> {
    let text = fs::read_to_string(path).ok()?;
    let text = text.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

fn number(path: &Path) -> Option<i64> {
    read(path)?.parse().ok()
}

/// The sensor's own label as the row's name, with `suffix_word` added when the label
/// doesn't already say it ("Coolant temp" stays, "edge" becomes "edge temperature").
fn label_text(
    label: Option<String>,
    fallback: String,
    word: &str,
    with_suffix: impl Fn(&str) -> String,
) -> String {
    match label {
        None => fallback,
        Some(l) if l.to_lowercase().contains(word) => l,
        Some(l) => with_suffix(&l),
    }
}

fn items(hwmon: &Path) -> Vec<Detail> {
    let mut items = Vec::new();
    for n in 1..=8 {
        let file = hwmon.join(format!("temp{n}_input"));
        let Some(milli) = number(&file) else { continue };
        let celsius = milli / 1000;
        let label = read(&hwmon.join(format!("temp{n}_label")));
        let crit = number(&hwmon.join(format!("temp{n}_crit"))).map(|c| c / 1000);
        let mut detail = Detail::new(
            label_text(label, crate::tr!("Temperature"), "temp", |l| {
                crate::tr!("{} temperature", l)
            }),
            crate::tr!("{} °C", celsius),
            file.display().to_string(),
        );
        detail = match crit {
            Some(crit) if crit > 0 && celsius >= crit - 5 => detail.verdict(
                Verdict::Reduced,
                Some(crate::tr!(
                    "This is within 5 °C of the highest temperature the maker allows ({} °C). Check that fans are spinning and vents are clear.",
                    crit
                )),
            ),
            _ => detail.verdict(Verdict::Info, None),
        };
        items.push(detail);
    }
    for n in 1..=8 {
        let file = hwmon.join(format!("fan{n}_input"));
        let Some(rpm) = number(&file) else { continue };
        let label = read(&hwmon.join(format!("fan{n}_label")));
        items.push(Detail::new(
            label_text(label, crate::tr!("Fan speed"), "speed", |l| {
                crate::tr!("{} speed", l)
            }),
            crate::tr!("{} rpm", rpm),
            file.display().to_string(),
        ));
    }
    for name in ["power1_average", "power1_input"] {
        let file = hwmon.join(name);
        if let Some(micro) = number(&file) {
            items.push(Detail::new(
                crate::tr!("Power use"),
                crate::tr!("{} W", micro / 1_000_000),
                file.display().to_string(),
            ));
            break;
        }
    }
    items
}

/// Add a "Sensors" group to each device a sensor belongs to (the device whose sysfs folder
/// is the longest match for where the sensor's `device` link points).
pub(crate) fn attach(devices: &mut [Device], hwmon_dir: &Path) {
    let Ok(entries) = fs::read_dir(hwmon_dir) else {
        return;
    };
    let mut sensors: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    sensors.sort();
    for hwmon in sensors {
        if read(&hwmon.join("name")).is_none_or(|n| SKIPPED.contains(&n.as_str())) {
            continue;
        }
        let Ok(owner) = fs::canonicalize(hwmon.join("device")) else {
            continue;
        };
        let found = devices
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                !d.sysfs_path.as_os_str().is_empty()
                    && fs::canonicalize(&d.sysfs_path).is_ok_and(|p| owner.starts_with(p))
            })
            .max_by_key(|(_, d)| d.sysfs_path.as_os_str().len())
            .map(|(i, _)| i);
        let (Some(index), items) = (found, items(&hwmon)) else {
            continue;
        };
        if items.is_empty() {
            continue;
        }
        let title = crate::tr!("Sensors");
        match devices[index].details.iter_mut().find(|g| g.title == title) {
            Some(group) => group.items.extend(items),
            None => devices[index].details.push(DetailGroup { title, items }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{BuiltIn, Category};
    use crate::i18n::in_english;
    use std::os::unix::fs::symlink;

    fn write(dir: &Path, name: &str, text: &str) {
        fs::write(dir.join(name), text).expect("write");
    }

    fn device(path: &Path, address: &str) -> Device {
        let mut d = Device::built_in(BuiltIn {
            address: "x",
            sysfs_path: path.to_path_buf(),
            display_name: address.to_owned(),
            vendor_name: None,
            category: Category::Graphics,
            hardware_id: String::new(),
            description: String::new(),
            details: Vec::new(),
        });
        d.address = address.to_owned();
        d
    }

    #[test]
    fn sensors_attach_to_the_device_they_belong_to_and_skip_covered_ones() {
        let root = tempfile::tempdir().expect("tempdir");
        let gpu = root.path().join("pci/0000:0c:00.0");
        let usb = root.path().join("pci/usb1/1-9");
        let hid = usb.join("1-9:1.0/hid0");
        for d in [&gpu, &hid] {
            fs::create_dir_all(d).expect("mkdir");
        }
        let hw = root.path().join("hwmon");
        for (n, name, target) in [
            ("hwmon0", "amdgpu", &gpu),
            ("hwmon1", "kraken", &hid),
            ("hwmon2", "k10temp", &gpu),
        ] {
            let dir = hw.join(n);
            fs::create_dir_all(&dir).expect("mkdir");
            write(&dir, "name", name);
            symlink(target, dir.join("device")).expect("symlink");
        }
        write(&hw.join("hwmon0"), "temp1_input", "62000\n");
        write(&hw.join("hwmon0"), "temp1_label", "edge\n");
        write(&hw.join("hwmon0"), "temp1_crit", "65000\n");
        write(&hw.join("hwmon0"), "power1_average", "21000000\n");
        write(&hw.join("hwmon1"), "temp1_input", "31000\n");
        write(&hw.join("hwmon1"), "fan1_input", "1200\n");
        write(&hw.join("hwmon1"), "fan1_label", "Pump\n");
        write(&hw.join("hwmon2"), "temp1_input", "50000\n");
        let mut devices = vec![device(&gpu, "gpu"), device(&usb, "cooler")];
        in_english(|| attach(&mut devices, &hw));
        let gpu_items = &devices[0].details[0].items;
        assert_eq!(devices[0].details[0].title, "Sensors");
        assert_eq!(gpu_items.len(), 2, "k10temp is skipped: {gpu_items:#?}");
        assert_eq!(gpu_items[0].label, "edge temperature");
        assert_eq!(gpu_items[0].value, "62 °C");
        assert_eq!(
            gpu_items[0].verdict,
            Verdict::Reduced,
            "within 5 °C of critical"
        );
        assert_eq!(gpu_items[1].value, "21 W");
        let cooler = &devices[1].details[0].items;
        assert_eq!(cooler[0].verdict, Verdict::Info);
        assert_eq!(cooler[1].label, "Pump speed");
        assert_eq!(cooler[1].value, "1200 rpm");
    }
}
