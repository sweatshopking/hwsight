//! Where hwsight finds its updatable data.
//!
//! Data comes in layers. For each file we look in:
//!
//! 1. The download folder, `$XDG_DATA_HOME/hwsight/data` (normally
//!    `~/.local/share/hwsight/data`), where hwsight's updater saves fresh copies.
//! 2. System data folders, `$XDG_DATA_DIRS/hwsight/data` (normally under `/usr/share`),
//!    where a distro package may install data.
//! 3. For device name lists only, the distro's own copies (from the `hwdata` package).
//!
//! The data built into the program is the final fallback, so hwsight always works offline.
//! Callers compare the versions of what they find and use the newest.

use std::env;
use std::path::{Path, PathBuf};

/// Where distros install the device name lists, most common first.
const DISTRO_ID_DIRS: &[&str] = &[
    "/usr/share/hwdata", // Arch, Fedora, openSUSE, newer Debian/Ubuntu
    "/usr/share/misc",   // Debian, Ubuntu
    "/usr/share",
    "/usr/local/share/hwdata",
];

#[derive(Debug, Clone)]
pub struct DataDirs {
    /// Where downloaded updates go. `None` if we can't work out a home folder.
    pub download: Option<PathBuf>,
    /// Package-installed hwsight data folders.
    pub system: Vec<PathBuf>,
    /// Distro folders holding `pci.ids` and `usb.ids`.
    pub distro_ids: Vec<PathBuf>,
}

impl DataDirs {
    /// The standard locations, following the XDG base directory spec.
    pub fn standard() -> Self {
        let download = env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .or_else(|| env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
            .map(|base| base.join("hwsight/data"));

        let data_dirs = env::var("XDG_DATA_DIRS")
            .ok()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| "/usr/local/share:/usr/share".to_owned());
        let system = data_dirs
            .split(':')
            .map(Path::new)
            .filter(|p| p.is_absolute())
            .map(|p| p.join("hwsight/data"))
            .collect();

        Self {
            download,
            system,
            distro_ids: DISTRO_ID_DIRS.iter().map(PathBuf::from).collect(),
        }
    }

    /// Only the given folder (used by tests and for pointing at a custom data set).
    pub fn only(dir: &Path) -> Self {
        Self {
            download: Some(dir.to_path_buf()),
            system: Vec::new(),
            distro_ids: Vec::new(),
        }
    }

    /// Every existing copy of hwsight data file `name`, download folder first.
    pub fn hwsight_files(&self, name: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> = self
            .download
            .iter()
            .chain(&self.system)
            .map(|dir| dir.join(name))
            .filter(|p| p.is_file())
            .collect();
        files.dedup();
        files
    }

    /// Every existing copy of a device name list such as `pci.ids`, including distro copies.
    pub fn id_files(&self, name: &str) -> Vec<PathBuf> {
        let mut files = self.hwsight_files(name);
        for path in self.distro_ids.iter().map(|dir| dir.join(name)) {
            // Skip symlinks to a file we already have (e.g. /usr/share/misc -> hwdata).
            let same = files.iter().any(|f| same_file(f, &path));
            if path.is_file() && !same {
                files.push(path);
            }
        }
        files
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------
// Versioned hwsight data files (rules.toml, firmware.toml, ...).
//
// Every file has `schema` (format version) and `version` (release date, YYYY.MM.DD).
// A copy is built into the program; a newer valid copy in a data folder replaces it as a
// whole (newer files are complete supersets, so no merging). Broken files and files for
// a newer hwsight are skipped and reported, never used half-way.

/// Where a data file in use came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataSource {
    Embedded,
    File(PathBuf),
}

/// Why a data file wasn't used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataProblem {
    /// Written for a newer hwsight; the user should update the app.
    NewerSchema { path: PathBuf, schema: u32 },
    /// Unreadable or broken.
    Invalid { path: PathBuf, error: String },
}

/// A hwsight data file type.
pub(crate) trait DataFile: Sized {
    /// File name in the data folders, e.g. `rules.toml`.
    const FILE_NAME: &'static str;
    /// The format version this build understands.
    const SCHEMA: u32;
    /// The built-in copy.
    const EMBEDDED: &'static str;
    /// Parse the whole file (schema already checked). Return its `version` and contents.
    fn parse_body(text: &str) -> Result<(String, Self), String>;
    /// Used only if the built-in copy is somehow broken: run without data, don't crash.
    fn empty() -> Self;
}

#[derive(Debug, Clone)]
pub(crate) struct Loaded<T> {
    pub data: T,
    pub version: String,
    pub source: DataSource,
}

#[derive(Debug)]
pub(crate) enum ParseError {
    NewerSchema(u32),
    Invalid(String),
}

pub(crate) fn parse_data<T: DataFile>(text: &str) -> Result<(String, T), ParseError> {
    // Check the schema first, so a newer file is reported as "newer" even if its other
    // fields have changed shape.
    #[derive(serde::Deserialize)]
    struct SchemaOnly {
        schema: u32,
    }
    let schema = toml::from_str::<SchemaOnly>(text)
        .map_err(|e| ParseError::Invalid(e.to_string()))?
        .schema;
    if schema > T::SCHEMA {
        return Err(ParseError::NewerSchema(schema));
    }
    if schema < T::SCHEMA {
        return Err(ParseError::Invalid(format!("old format (schema {schema})")));
    }
    let (version, data) = T::parse_body(text).map_err(ParseError::Invalid)?;
    if !is_date_version(&version) {
        // Versions are compared as text to pick the newest, which needs one fixed format.
        return Err(ParseError::Invalid(format!(
            "version `{version}` should look like 2026.11.02"
        )));
    }
    Ok((version, data))
}

pub(crate) fn embedded<T: DataFile>() -> Loaded<T> {
    match parse_data::<T>(T::EMBEDDED) {
        Ok((version, data)) => Loaded {
            data,
            version,
            source: DataSource::Embedded,
        },
        // Tests check every built-in file, so this can't happen in a release.
        Err(_) => Loaded {
            data: T::empty(),
            version: String::new(),
            source: DataSource::Embedded,
        },
    }
}

/// The newest valid copy from the data folders or the built-in one, plus skipped files.
pub(crate) fn load_newest<T: DataFile>(dirs: &DataDirs) -> (Loaded<T>, Vec<DataProblem>) {
    let mut best = embedded::<T>();
    let mut problems = Vec::new();
    for path in dirs.hwsight_files(T::FILE_NAME) {
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                problems.push(DataProblem::Invalid {
                    path,
                    error: e.to_string(),
                });
                continue;
            }
        };
        match parse_data::<T>(&text) {
            Ok((version, data)) if version > best.version => {
                best = Loaded {
                    data,
                    version,
                    source: DataSource::File(path),
                };
            }
            Ok(_) => {}
            Err(ParseError::NewerSchema(schema)) => {
                problems.push(DataProblem::NewerSchema { path, schema });
            }
            Err(ParseError::Invalid(error)) => problems.push(DataProblem::Invalid { path, error }),
        }
    }
    (best, problems)
}

/// `YYYY.MM.DD`, the format shared by every hwsight data file and `pci.ids`.
pub(crate) fn is_date_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() == 10
        && b[4] == b'.'
        && b[7] == b'.'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}
