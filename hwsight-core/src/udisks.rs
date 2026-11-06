//! Drive health from the UDisks2 service over D-Bus.
//!
//! UDisks2 runs as root and lets any logged-in user read a drive's saved SMART health
//! data, so hwsight needs no password. Reading drive health this way (cached values, no
//! self-tests, nothing written) follows QLinuxDeviceManager (GPL-3.0).
//!
//! This file only turns D-Bus data into plain values. Judging them is in `drives.rs`, and
//! is tested there with made-up values.

use std::collections::HashMap;
use std::time::Duration;

use zbus::blocking::Proxy;
use zbus::blocking::connection::Builder;
use zbus::blocking::fdo::ObjectManagerProxy;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

const SERVICE: &str = "org.freedesktop.UDisks2";
const ROOT: &str = "/org/freedesktop/UDisks2";
const BLOCK: &str = "org.freedesktop.UDisks2.Block";
const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
const ATA: &str = "org.freedesktop.UDisks2.Drive.Ata";
const NVME: &str = "org.freedesktop.UDisks2.NVMe.Controller";

/// How long to wait for any one answer. A stuck service must never freeze a scan.
const TIMEOUT: Duration = Duration::from_secs(4);

/// One ATA (SATA/IDE) SMART attribute.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AtaAttribute {
    pub id: u8,
    pub name: String,
    pub value: i32,
    pub worst: i32,
    pub threshold: i32,
    /// The raw counter the drive reports.
    pub raw: i64,
}

/// What UDisks2 knows about a SATA or IDE drive's health.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct AtaHealth {
    pub failing: bool,
    pub bad_sectors: i64,
    pub attributes_failing: i32,
    pub attributes_failed_in_the_past: i32,
    pub power_on_seconds: u64,
    /// In kelvin, as UDisks2 reports it; 0 when unknown.
    pub temperature_k: f64,
    pub attributes: Vec<AtaAttribute>,
}

/// What UDisks2 knows about an NVMe drive's health.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct NvmeHealth {
    pub critical_warning: Vec<String>,
    pub power_on_hours: u64,
    pub temperature_k: u16,
    /// The drive's own numbers (percent_used, avail_spare, total_data_written, ...).
    pub counters: HashMap<String, u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Health {
    Ata(AtaHealth),
    Nvme(NvmeHealth),
}

/// A whole drive as UDisks2 describes it, found by its block device name ("nvme0n1").
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct UdisksDrive {
    pub block: String,
    /// 0 for solid-state drives, otherwise the spin speed in rpm.
    pub rotation_rate: i32,
    pub health: Option<Health>,
}

type Properties = HashMap<String, OwnedValue>;

fn clone_value(value: &OwnedValue) -> Option<OwnedValue> {
    value.try_clone().ok()
}

fn as_u64(value: &OwnedValue) -> Option<u64> {
    let v: &Value = value;
    match v {
        Value::U8(n) => Some(u64::from(*n)),
        Value::U16(n) => Some(u64::from(*n)),
        Value::U32(n) => Some(u64::from(*n)),
        Value::U64(n) => Some(*n),
        Value::I32(n) => u64::try_from(*n).ok(),
        Value::I64(n) => u64::try_from(*n).ok(),
        _ => None,
    }
}

fn as_bool(value: &OwnedValue) -> Option<bool> {
    bool::try_from(clone_value(value)?).ok()
}

fn as_i64(value: &OwnedValue) -> Option<i64> {
    i64::try_from(clone_value(value)?).ok()
}

fn as_f64(value: &OwnedValue) -> Option<f64> {
    f64::try_from(clone_value(value)?).ok()
}

fn as_strings(value: &OwnedValue) -> Vec<String> {
    clone_value(value)
        .and_then(|v| Vec::<String>::try_from(v).ok())
        .unwrap_or_default()
}

fn get_u64(props: &Properties, key: &str) -> u64 {
    props.get(key).and_then(as_u64).unwrap_or(0)
}

fn ata_health(props: &Properties, attributes: Vec<AtaAttribute>) -> AtaHealth {
    AtaHealth {
        failing: props.get("SmartFailing").and_then(as_bool).unwrap_or(false),
        bad_sectors: props
            .get("SmartNumBadSectors")
            .and_then(as_i64)
            .unwrap_or(0),
        attributes_failing: props
            .get("SmartNumAttributesFailing")
            .and_then(as_i64)
            .unwrap_or(0) as i32,
        attributes_failed_in_the_past: props
            .get("SmartNumAttributesFailedInThePast")
            .and_then(as_i64)
            .unwrap_or(0) as i32,
        power_on_seconds: get_u64(props, "SmartPowerOnSeconds"),
        temperature_k: props
            .get("SmartTemperature")
            .and_then(as_f64)
            .unwrap_or(0.0),
        attributes,
    }
}

/// SATA attributes: an array of (id, name, flags, value, worst, threshold, raw, ...).
fn parse_ata_attributes(
    connection: &zbus::blocking::Connection,
    path: &OwnedObjectPath,
) -> Vec<AtaAttribute> {
    let Ok(proxy) = Proxy::new(connection, SERVICE, path.as_str(), ATA) else {
        return Vec::new();
    };
    let options: HashMap<&str, Value> = HashMap::new();
    let Ok(reply) = proxy.call_method("SmartGetAttributes", &(options,)) else {
        return Vec::new();
    };
    type Row = (u8, String, u16, i32, i32, i32, i64, i32, Properties);
    let Ok(rows) = reply.body().deserialize::<Vec<Row>>() else {
        return Vec::new();
    };
    rows.into_iter()
        .map(
            |(id, name, _flags, value, worst, threshold, pretty, _unit, _extra)| AtaAttribute {
                id,
                name,
                value,
                worst,
                threshold,
                raw: pretty,
            },
        )
        .collect()
}

fn nvme_counters(
    connection: &zbus::blocking::Connection,
    path: &OwnedObjectPath,
) -> HashMap<String, u64> {
    let Ok(proxy) = Proxy::new(connection, SERVICE, path.as_str(), NVME) else {
        return HashMap::new();
    };
    let options: HashMap<&str, Value> = HashMap::new();
    let Ok(reply) = proxy.call_method("SmartGetAttributes", &(options,)) else {
        return HashMap::new();
    };
    reply
        .body()
        .deserialize::<Properties>()
        .map(|map| {
            map.iter()
                .filter_map(|(k, v)| Some((k.clone(), as_u64(v)?)))
                .collect()
        })
        .unwrap_or_default()
}

/// Every whole drive UDisks2 knows about, or an empty list when the service isn't
/// running (or the system bus isn't available, as in a container).
pub(crate) fn read_drives() -> Vec<UdisksDrive> {
    let Ok(builder) = Builder::system() else {
        return Vec::new();
    };
    let Ok(connection) = builder.method_timeout(TIMEOUT).build() else {
        return Vec::new();
    };
    let Ok(manager) = ObjectManagerProxy::builder(&connection)
        .destination(SERVICE)
        .and_then(|b| b.path(ROOT))
        .and_then(|b| b.build())
    else {
        return Vec::new();
    };
    let Ok(objects) = manager.get_managed_objects() else {
        return Vec::new();
    };

    // Which drive object each whole-disk block device belongs to.
    let mut blocks: Vec<(String, OwnedObjectPath)> = Vec::new();
    for (path, interfaces) in &objects {
        if interfaces.contains_key("org.freedesktop.UDisks2.Partition") {
            continue;
        }
        let Some(block) = interfaces.get(BLOCK) else {
            continue;
        };
        let Some(drive) = block
            .get("Drive")
            .and_then(clone_value)
            .and_then(|v| OwnedObjectPath::try_from(v).ok())
            .filter(|p| p.as_str() != "/")
        else {
            continue;
        };
        if let Some(name) = path.as_str().rsplit('/').next() {
            blocks.push((name.to_owned(), drive));
        }
    }
    blocks.sort_by(|a, b| a.0.cmp(&b.0));

    blocks
        .into_iter()
        .filter_map(|(block, drive_path)| {
            let interfaces = objects.iter().find(|(p, _)| **p == drive_path)?.1;
            let drive = interfaces.get(DRIVE)?;
            let rotation_rate = drive.get("RotationRate").and_then(as_i64).unwrap_or(0) as i32;
            let health = if let Some(nvme) = interfaces.get(NVME) {
                Some(Health::Nvme(NvmeHealth {
                    critical_warning: nvme
                        .get("SmartCriticalWarning")
                        .map(as_strings)
                        .unwrap_or_default(),
                    power_on_hours: get_u64(nvme, "SmartPowerOnHours"),
                    temperature_k: get_u64(nvme, "SmartTemperature") as u16,
                    counters: nvme_counters(&connection, &drive_path),
                }))
            } else if let Some(ata) = interfaces.get(ATA) {
                ata.get("SmartEnabled")
                    .and_then(as_bool)
                    .unwrap_or(false)
                    .then(|| {
                        Health::Ata(ata_health(
                            ata,
                            parse_ata_attributes(&connection, &drive_path),
                        ))
                    })
            } else {
                None
            };
            Some(UdisksDrive {
                block,
                rotation_rate,
                health,
            })
        })
        .collect()
}
