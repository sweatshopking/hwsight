//! Firmware information from the fwupd service over D-Bus.
//!
//! fwupd lists the devices whose firmware it knows about (drives, wireless receivers, the
//! BIOS) and, from a downloaded catalog (the LVFS), the newer versions available. hwsight
//! only reads: nothing is installed or downloaded by this file, and no serial numbers are
//! kept. The catalog's state is read too, because "no updates" means nothing when the
//! catalog was never downloaded.

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use zbus::blocking::Proxy;
use zbus::blocking::connection::Builder;
use zbus::zvariant::{OwnedValue, Value};

const SERVICE: &str = "org.freedesktop.fwupd";
const TIMEOUT: Duration = Duration::from_secs(4);

/// A catalog older than this is called out as out of date.
const STALE_AFTER_DAYS: u64 = 30;

/// Whether fwupd has a recent list of available updates to compare against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Catalog {
    Fresh,
    /// Downloaded, but this many days ago.
    Stale {
        days: u64,
    },
    /// A download source is switched on but nothing was ever downloaded from it.
    NeverDownloaded,
    /// No download source is switched on.
    NoSource,
}

/// A newer firmware version for a device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Update {
    pub version: String,
    pub summary: Option<String>,
}

/// One device fwupd knows about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FwupdDevice {
    pub id: String,
    /// The device this one is reached through (a paired keyboard's receiver).
    pub parent: Option<String>,
    pub name: String,
    pub version: Option<String>,
    pub plugin: String,
    /// fwupd can install firmware for it.
    pub updatable: bool,
    /// Identifiers like `NVME\VEN_C0A9&DEV_5427` or `USB\VID_046D&PID_C548`.
    pub ids: Vec<String>,
    /// (level percent, low-battery threshold percent) for wireless devices.
    pub battery: Option<(u64, u64)>,
    pub update: Option<Update>,
    /// An update was installed but only takes effect after a restart.
    pub needs_restart: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Fwupd {
    pub catalog: Catalog,
    pub devices: Vec<FwupdDevice>,
}

type Properties = HashMap<String, OwnedValue>;

fn text(props: &Properties, key: &str) -> Option<String> {
    let value = props.get(key)?.try_clone().ok()?;
    let text = String::try_from(value).ok()?;
    let text = text.trim().to_owned();
    (!text.is_empty()).then_some(text)
}

fn number(props: &Properties, key: &str) -> Option<u64> {
    let value: &Value = props.get(key)?;
    match value {
        Value::U8(n) => Some(u64::from(*n)),
        Value::U16(n) => Some(u64::from(*n)),
        Value::U32(n) => Some(u64::from(*n)),
        Value::U64(n) => Some(*n),
        _ => None,
    }
}

fn strings(props: &Properties, key: &str) -> Vec<String> {
    props
        .get(key)
        .and_then(|v| v.try_clone().ok())
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default()
}

/// fwupd's update state for a device that was updated and is waiting for a restart.
const UPDATE_STATE_NEEDS_REBOOT: u64 = 4;

/// fwupd's device flags: bit 1 means "can be updated".
const FLAG_UPDATABLE: u64 = 1 << 1;

pub(crate) fn device_from(props: &Properties) -> Option<FwupdDevice> {
    let level = number(props, "BatteryLevel").filter(|l| *l <= 100);
    Some(FwupdDevice {
        id: text(props, "DeviceId")?,
        parent: text(props, "ParentDeviceId"),
        name: text(props, "Name")?,
        version: text(props, "Version"),
        plugin: text(props, "Plugin").unwrap_or_default(),
        updatable: number(props, "Flags").is_some_and(|f| f & FLAG_UPDATABLE != 0),
        ids: strings(props, "InstanceIds"),
        battery: level.map(|l| (l, number(props, "BatteryThreshold").unwrap_or(0))),
        update: None,
        needs_restart: number(props, "UpdateState") == Some(UPDATE_STATE_NEEDS_REBOOT),
    })
}

fn update_from(props: &Properties) -> Option<Update> {
    Some(Update {
        version: text(props, "Version")?,
        summary: text(props, "Summary"),
    })
}

/// The state of the catalog, from fwupd's list of sources: each has a "Type" (1 means it
/// downloads), "Enabled", and the time it was last refreshed ("ModificationTime", which is
/// the largest possible number when it never was).
pub(crate) fn catalog_from(remotes: &[Properties], now: u64) -> Catalog {
    let downloads: Vec<&Properties> = remotes
        .iter()
        .filter(|r| number(r, "Type") == Some(1))
        .filter(|r| {
            r.get("Enabled")
                .and_then(|v| bool::try_from(v.try_clone().ok()?).ok())
                == Some(true)
        })
        .collect();
    if downloads.is_empty() {
        return Catalog::NoSource;
    }
    let newest = downloads
        .iter()
        .filter_map(|r| number(r, "ModificationTime"))
        .filter(|t| *t < u64::MAX / 2)
        .max();
    match newest {
        None => Catalog::NeverDownloaded,
        Some(t) => {
            let days = now.saturating_sub(t) / 86_400;
            if days > STALE_AFTER_DAYS {
                Catalog::Stale { days }
            } else {
                Catalog::Fresh
            }
        }
    }
}

/// Ask fwupd about its catalog, its devices, and available updates. `None` when the
/// service isn't there (not installed, or no system bus as in a container).
pub(crate) fn read() -> Option<Fwupd> {
    let connection = Builder::system()
        .ok()?
        .method_timeout(TIMEOUT)
        .build()
        .ok()?;
    let proxy = Proxy::new(&connection, SERVICE, "/", SERVICE).ok()?;
    let rows = |name: &str| -> Option<Vec<Properties>> {
        proxy
            .call_method(name, &())
            .ok()?
            .body()
            .deserialize::<Vec<Properties>>()
            .ok()
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let catalog = rows("GetRemotes").map_or(Catalog::NoSource, |r| catalog_from(&r, now));
    let mut devices: Vec<FwupdDevice> =
        rows("GetDevices")?.iter().filter_map(device_from).collect();

    // Updates can only be looked up once there's a catalog (without one fwupd answers every
    // question with an error).
    if matches!(catalog, Catalog::Fresh | Catalog::Stale { .. }) {
        for device in devices.iter_mut().filter(|d| d.updatable) {
            let releases = proxy
                .call_method("GetUpgrades", &(device.id.as_str(),))
                .ok()
                .and_then(|reply| reply.body().deserialize::<Vec<Properties>>().ok());
            // The newest release comes first.
            device.update = releases.and_then(|r| r.first().and_then(update_from));
        }
    }
    Some(Fwupd { catalog, devices })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn props(entries: Vec<(&str, Value<'_>)>) -> Properties {
        entries
            .into_iter()
            .map(|(k, v)| (k.to_owned(), OwnedValue::try_from(v).expect("value")))
            .collect()
    }

    fn remote(kind: u32, enabled: bool, modified: u64) -> Properties {
        props(vec![
            ("Type", Value::U32(kind)),
            ("Enabled", Value::Bool(enabled)),
            ("ModificationTime", Value::U64(modified)),
        ])
    }

    const NOW: u64 = 1_800_000_000;
    const DAY: u64 = 86_400;

    #[test]
    fn a_device_is_read_with_its_ids_battery_and_parent() {
        let keyboard = device_from(&props(vec![
            ("DeviceId", Value::from("abc")),
            ("Name", Value::from("Alto Keys K98")),
            ("Version", Value::from("RBK93.02_B0009")),
            ("Plugin", Value::from("logitech_hidpp")),
            ("ParentDeviceId", Value::from("receiver")),
            ("Flags", Value::U64(0x10_0000_0000_0002)),
            ("BatteryLevel", Value::U32(100)),
            ("BatteryThreshold", Value::U32(20)),
            (
                "InstanceIds",
                Value::from(vec!["HIDRAW\\VEN_046D&MOD_B38E00000000".to_owned()]),
            ),
        ]))
        .expect("device");
        assert_eq!(keyboard.name, "Alto Keys K98");
        assert_eq!(keyboard.parent.as_deref(), Some("receiver"));
        assert!(keyboard.updatable, "bit 1 of the flags");
        assert_eq!(keyboard.battery, Some((100, 20)));
        assert_eq!(keyboard.ids, ["HIDRAW\\VEN_046D&MOD_B38E00000000"]);
        assert_eq!(keyboard.update, None);
        assert!(!keyboard.needs_restart);

        let fixed = device_from(&props(vec![
            ("DeviceId", Value::from("x")),
            ("Name", Value::from("TPM")),
            ("Flags", Value::U64(1)),
            ("UpdateState", Value::U32(4)),
        ]))
        .expect("device");
        assert!(fixed.needs_restart, "update state 4 is needs-reboot");
        assert!(!fixed.updatable, "internal only");
        assert_eq!(fixed.battery, None);
        assert_eq!(fixed.version, None);
        assert_eq!(
            device_from(&props(vec![("Name", Value::from("no id"))])),
            None
        );
    }

    #[test]
    fn an_update_needs_a_version() {
        let update = update_from(&props(vec![
            ("Version", Value::from("2.0")),
            ("Summary", Value::from("Fixes a bug")),
        ]))
        .expect("update");
        assert_eq!(update.version, "2.0");
        assert_eq!(update.summary.as_deref(), Some("Fixes a bug"));
        assert_eq!(
            update_from(&props(vec![("Summary", Value::from("x"))])),
            None
        );
    }

    #[test]
    fn the_catalog_state_comes_from_the_download_sources() {
        let never = u64::MAX;
        // The owner's PC: LVFS switched on but never downloaded; a local source is fresh
        // but doesn't download anything.
        assert_eq!(
            catalog_from(&[remote(3, true, NOW - DAY), remote(1, true, never)], NOW),
            Catalog::NeverDownloaded
        );
        assert_eq!(
            catalog_from(&[remote(1, true, NOW - 2 * DAY)], NOW),
            Catalog::Fresh
        );
        assert_eq!(
            catalog_from(&[remote(1, true, NOW - 45 * DAY)], NOW),
            Catalog::Stale { days: 45 }
        );
        assert_eq!(
            catalog_from(&[remote(1, false, NOW - DAY), remote(3, true, NOW)], NOW),
            Catalog::NoSource,
            "a switched-off source doesn't count"
        );
        assert_eq!(
            catalog_from(&[remote(1, true, never), remote(1, true, NOW - DAY)], NOW),
            Catalog::Fresh,
            "one fresh source is enough"
        );
        assert_eq!(catalog_from(&[], NOW), Catalog::NoSource);
    }
}
