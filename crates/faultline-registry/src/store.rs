//! Registered versions on disk.
//!
//! Layout, under the registry's directory:
//!
//! ```text
//! subjects/<subject>/v1.json      one record per version
//! subjects/<subject>/v2.json
//! subjects/<subject>/config.json  the subject's compatibility settings
//! ```
//!
//! A record holds every file of the version as text, so file paths chosen
//! by clients never become paths on disk; only validated subject names do.
//! Records are written to a temporary file and renamed into place, and
//! never modified afterwards. One process serves a directory: version
//! numbers are assigned under a lock held in memory.

use std::collections::BTreeMap;
use std::fmt;
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use faultline_policy::{ApiFormat, FailOn, SchemaRole};
use serde_json::{Value, json};

/// The most files one version may have.
pub const MAX_FILES: usize = 1000;
/// The longest file path within a version, in bytes.
pub const MAX_PATH_BYTES: usize = 512;
/// The longest subject name, in bytes.
pub const MAX_SUBJECT_BYTES: usize = 128;

/// The files of one version of an API description.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Bundle {
    /// The entry document, one of `files`.
    pub entry: String,
    /// Detected from the entry document when `None`.
    pub format: Option<ApiFormat>,
    pub role: SchemaRole,
    /// Text by relative path, `/`-separated.
    pub files: BTreeMap<String, String>,
}

impl Bundle {
    /// Reads a bundle from its JSON form, validating every path.
    pub fn from_json(value: &Value) -> Result<Bundle, String> {
        let object = value.as_object().ok_or("the body must be a JSON object")?;
        for key in object.keys() {
            if !matches!(key.as_str(), "entry" | "format" | "role" | "files") {
                return Err(format!("unknown field `{key}`"));
            }
        }
        let entry = object
            .get("entry")
            .and_then(Value::as_str)
            .ok_or("`entry` must name the entry document")?
            .to_owned();
        let format = match object.get("format") {
            None | Some(Value::Null) => None,
            Some(Value::String(name)) => {
                Some(ApiFormat::parse(name).ok_or_else(|| format!("unknown format `{name}`"))?)
            }
            Some(_) => return Err("`format` must be a string".into()),
        };
        let role = match object.get("role") {
            None | Some(Value::Null) => SchemaRole::Both,
            Some(Value::String(name)) => {
                SchemaRole::parse(name).ok_or_else(|| format!("unknown role `{name}`"))?
            }
            Some(_) => return Err("`role` must be a string".into()),
        };
        let files = object
            .get("files")
            .and_then(Value::as_object)
            .ok_or("`files` must map paths to their text")?;
        if files.is_empty() {
            return Err("`files` is empty".into());
        }
        if files.len() > MAX_FILES {
            return Err(format!("a version may have at most {MAX_FILES} files"));
        }
        let mut texts = BTreeMap::new();
        for (path, text) in files {
            validate_path(path)?;
            let text = text
                .as_str()
                .ok_or_else(|| format!("the text of `{path}` must be a string"))?;
            texts.insert(path.clone(), text.to_owned());
        }
        if !texts.contains_key(&entry) {
            return Err(format!("the entry document `{entry}` is not among `files`"));
        }
        Ok(Bundle {
            entry,
            format,
            role,
            files: texts,
        })
    }

    pub fn to_json(&self) -> Value {
        let mut value = json!({
            "entry": self.entry,
            "role": self.role.as_str(),
            "files": self.files,
        });
        if let Some(format) = self.format {
            value["format"] = json!(format.as_str());
        }
        value
    }
}

/// Checks a file path within a bundle: relative, `/`-separated, with no
/// empty, `.` or `..` segments and nothing a filesystem or URI would read
/// specially.
pub fn validate_path(path: &str) -> Result<(), String> {
    let bad = |why: &str| Err(format!("invalid file path `{path}`: {why}"));
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return bad(&format!("paths are 1 to {MAX_PATH_BYTES} bytes"));
    }
    if path
        .chars()
        .any(|c| c.is_control() || matches!(c, '\\' | ':' | '?' | '#' | '%'))
    {
        return bad("control characters, `\\`, `:`, `?`, `#` and `%` are not allowed");
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return bad("segments must be names, not empty, `.` or `..`");
        }
    }
    Ok(())
}

/// Checks a subject name: 1 to 128 of `A-Z a-z 0-9 . _ -`, not starting
/// with `.`.
pub fn validate_subject(subject: &str) -> Result<(), String> {
    let valid = !subject.is_empty()
        && subject.len() <= MAX_SUBJECT_BYTES
        && !subject.starts_with('.')
        && subject
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if valid {
        Ok(())
    } else {
        Err(format!(
            "invalid subject `{subject}`: use 1 to {MAX_SUBJECT_BYTES} letters, digits, `.`, `_` \
             and `-`, not starting with `.`"
        ))
    }
}

/// A registered version.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub version: u32,
    /// The date it was registered, `YYYY-MM-DD`.
    pub registered: String,
    /// The version the description declares (`info.version`), if any.
    pub api_version: Option<String>,
    pub bundle: Bundle,
}

impl Record {
    pub fn to_json(&self) -> Value {
        json!({
            "version": self.version,
            "registered": self.registered,
            "api_version": self.api_version,
            "bundle": self.bundle.to_json(),
        })
    }

    fn from_json(value: &Value) -> Result<Record, String> {
        let version = value["version"]
            .as_u64()
            .and_then(|v| u32::try_from(v).ok())
            .ok_or("missing version")?;
        Ok(Record {
            version,
            registered: value["registered"].as_str().unwrap_or_default().to_owned(),
            api_version: value["api_version"].as_str().map(str::to_owned),
            bundle: Bundle::from_json(&value["bundle"])?,
        })
    }
}

/// How a subject's new versions are checked.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compatibility {
    /// Findings of this severity or worse refuse a version.
    pub fail_on: FailOn,
    /// Check against every earlier version, not only the latest.
    pub transitive: bool,
}

impl Default for Compatibility {
    fn default() -> Self {
        Self {
            fail_on: FailOn::Breaking,
            transitive: false,
        }
    }
}

impl Compatibility {
    pub fn to_json(self) -> Value {
        json!({"fail_on": self.fail_on.as_str(), "transitive": self.transitive})
    }

    /// Reads settings, taking unspecified fields from `base`.
    pub fn from_json(value: &Value, base: Compatibility) -> Result<Compatibility, String> {
        let object = value.as_object().ok_or("the body must be a JSON object")?;
        let mut result = base;
        for (key, field) in object {
            match key.as_str() {
                "fail_on" => {
                    result.fail_on = field
                        .as_str()
                        .and_then(FailOn::parse)
                        .ok_or("`fail_on` is one of breaking, risky and never")?;
                }
                "transitive" => {
                    result.transitive = field.as_bool().ok_or("`transitive` is a boolean")?;
                }
                _ => return Err(format!("unknown field `{key}`")),
            }
        }
        Ok(result)
    }
}

/// Why a store operation failed.
#[derive(Debug)]
pub enum StoreError {
    Invalid(String),
    NotFound(String),
    /// The subject name differs only in case from an existing one, which
    /// would share its directory on case-insensitive filesystems.
    Conflict(String),
    Io(io::Error),
    /// A record on disk cannot be read.
    Corrupt(String),
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            StoreError::Invalid(message)
            | StoreError::NotFound(message)
            | StoreError::Conflict(message)
            | StoreError::Corrupt(message) => f.write_str(message),
            StoreError::Io(e) => write!(f, "storage error: {e}"),
        }
    }
}

impl std::error::Error for StoreError {}

impl From<io::Error> for StoreError {
    fn from(e: io::Error) -> Self {
        StoreError::Io(e)
    }
}

/// The registry's storage.
#[derive(Debug)]
pub struct Store {
    subjects: PathBuf,
    /// Held while assigning version numbers and writing settings.
    write: Mutex<()>,
}

impl Store {
    /// Opens (creating if needed) the store in `dir`.
    pub fn open(dir: impl AsRef<Path>) -> io::Result<Store> {
        let subjects = dir.as_ref().join("subjects");
        std::fs::create_dir_all(&subjects)?;
        Ok(Store {
            subjects,
            write: Mutex::new(()),
        })
    }

    /// Every subject with at least one version, sorted.
    pub fn subjects(&self) -> Result<Vec<String>, StoreError> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.subjects)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if validate_subject(&name).is_ok() && !self.versions(&name)?.is_empty() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    fn dir(&self, subject: &str) -> Result<PathBuf, StoreError> {
        validate_subject(subject).map_err(StoreError::Invalid)?;
        Ok(self.subjects.join(subject))
    }

    /// The subject's version numbers, ascending; empty for an unknown
    /// subject.
    pub fn versions(&self, subject: &str) -> Result<Vec<u32>, StoreError> {
        let dir = self.dir(subject)?;
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(e.into()),
        };
        let mut versions = Vec::new();
        for entry in entries {
            let name = entry?.file_name();
            let Some(name) = name.to_str() else {
                continue;
            };
            if let Some(number) = name
                .strip_prefix('v')
                .and_then(|rest| rest.strip_suffix(".json"))
                .filter(|n| !n.starts_with('0'))
                .and_then(|n| n.parse::<u32>().ok())
            {
                versions.push(number);
            }
        }
        versions.sort_unstable();
        Ok(versions)
    }

    pub fn get(&self, subject: &str, version: u32) -> Result<Record, StoreError> {
        let path = self.dir(subject)?.join(format!("v{version}.json"));
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(StoreError::NotFound(format!(
                    "{subject} has no version {version}"
                )));
            }
            Err(e) => return Err(e.into()),
        };
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| StoreError::Corrupt(format!("{}: {e}", path.display())))?;
        Record::from_json(&value)
            .map_err(|e| StoreError::Corrupt(format!("{}: {e}", path.display())))
    }

    pub fn latest(&self, subject: &str) -> Result<Option<Record>, StoreError> {
        match self.versions(subject)?.last() {
            Some(&version) => self.get(subject, version).map(Some),
            None => Ok(None),
        }
    }

    /// Stores the next version of `subject`, after `admit` accepts it
    /// given the versions registered so far (ascending). Holding the write
    /// lock throughout keeps two registrations from checking against the
    /// same history and both succeeding.
    pub fn register<E>(
        &self,
        subject: &str,
        registered: &str,
        admit: impl FnOnce(&[Record]) -> Result<Admission, E>,
    ) -> Result<Result<Registered, E>, StoreError> {
        let dir = self.dir(subject)?;
        let _guard = self
            .write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check_case(subject)?;
        let history = self
            .versions(subject)?
            .into_iter()
            .map(|v| self.get(subject, v))
            .collect::<Result<Vec<_>, _>>()?;
        let admission = match admit(&history) {
            Ok(admission) => admission,
            Err(e) => return Ok(Err(e)),
        };
        if let Some(latest) = history.last()
            && latest.bundle == admission.bundle
        {
            return Ok(Ok(Registered {
                record: latest.clone(),
                created: false,
            }));
        }
        let version = history.last().map_or(1, |r| r.version + 1);
        let record = Record {
            version,
            registered: registered.to_owned(),
            api_version: admission.api_version,
            bundle: admission.bundle,
        };
        std::fs::create_dir_all(&dir)?;
        write_atomically(&dir.join(format!("v{version}.json")), &record.to_json())?;
        Ok(Ok(Registered {
            record,
            created: true,
        }))
    }

    /// The subject's settings, or `default` when none were set.
    pub fn compatibility(
        &self,
        subject: &str,
        default: Compatibility,
    ) -> Result<Compatibility, StoreError> {
        let path = self.dir(subject)?.join("config.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let value: Value = serde_json::from_str(&text)
                    .map_err(|e| StoreError::Corrupt(format!("{}: {e}", path.display())))?;
                Compatibility::from_json(&value, default)
                    .map_err(|e| StoreError::Corrupt(format!("{}: {e}", path.display())))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(default),
            Err(e) => Err(e.into()),
        }
    }

    pub fn set_compatibility(
        &self,
        subject: &str,
        settings: Compatibility,
    ) -> Result<(), StoreError> {
        let dir = self.dir(subject)?;
        let _guard = self
            .write
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.check_case(subject)?;
        std::fs::create_dir_all(&dir)?;
        write_atomically(&dir.join("config.json"), &settings.to_json())?;
        Ok(())
    }

    /// Refuses a subject whose name differs only in case from an existing
    /// one.
    fn check_case(&self, subject: &str) -> Result<(), StoreError> {
        for entry in std::fs::read_dir(&self.subjects)? {
            let name = entry?.file_name();
            if let Some(name) = name.to_str()
                && name != subject
                && name.eq_ignore_ascii_case(subject)
            {
                return Err(StoreError::Conflict(format!(
                    "subject `{subject}` differs only in case from `{name}`"
                )));
            }
        }
        Ok(())
    }
}

/// What `admit` decides to store.
#[derive(Debug)]
pub struct Admission {
    pub bundle: Bundle,
    pub api_version: Option<String>,
}

/// The outcome of a registration.
#[derive(Debug)]
pub struct Registered {
    pub record: Record,
    /// `false` when the bundle was identical to the latest version, which
    /// is returned instead of a new one.
    pub created: bool,
}

fn write_atomically(path: &Path, value: &Value) -> io::Result<()> {
    let mut text = serde_json::to_vec_pretty(value).expect("JSON values always serialize");
    text.push(b'\n');
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let result = (|| {
        let mut file = std::fs::File::create(&temporary)?;
        file.write_all(&text)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(text: &str) -> Bundle {
        Bundle::from_json(&json!({"entry": "api.yaml", "files": {"api.yaml": text}})).unwrap()
    }

    #[test]
    fn paths_and_subjects() {
        for good in [
            "a.yaml",
            "specs/v1/pets.proto",
            "a b/c-d_e.json",
            "é.graphql",
        ] {
            assert!(validate_path(good).is_ok(), "{good}");
        }
        for bad in [
            "", "/a", "a/", "a//b", "./a", "a/../b", "..", "a\\b", "c:/a", "a?b", "a#b", "a%2e",
            "a\nb",
        ] {
            assert!(validate_path(bad).is_err(), "{bad:?}");
        }
        assert!(validate_path(&"a".repeat(MAX_PATH_BYTES + 1)).is_err());
        for good in ["pets", "pets.v1", "Order_Service-2"] {
            assert!(validate_subject(good).is_ok(), "{good}");
        }
        for bad in ["", ".hidden", "..", "a/b", "a b", "é", &"a".repeat(129)] {
            assert!(validate_subject(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bundles() {
        let b = Bundle::from_json(&json!({
            "entry": "a.proto",
            "format": "protobuf",
            "role": "input",
            "files": {"a.proto": "x", "b/c.proto": "y"},
        }))
        .unwrap();
        assert_eq!(b.format, Some(ApiFormat::Protobuf));
        assert_eq!(Bundle::from_json(&b.to_json()).unwrap(), b);
        for (bad, why) in [
            (json!([]), "object"),
            (json!({"entry": "a", "files": {}}), "empty"),
            (json!({"entry": "a", "files": {"b": "x"}}), "not among"),
            (json!({"entry": "a", "files": {"a": 1}}), "must be a string"),
            (
                json!({"entry": "a", "files": {"a": "x"}, "format": "wsdl"}),
                "unknown format",
            ),
            (
                json!({"entry": "a", "files": {"a": "x"}, "extra": 1}),
                "unknown field",
            ),
            (
                json!({"entry": "../a", "files": {"../a": "x"}}),
                "invalid file path",
            ),
        ] {
            let error = Bundle::from_json(&bad).unwrap_err();
            assert!(error.contains(why), "{error}");
        }
    }

    #[test]
    fn versions_are_numbered_and_deduplicated() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        assert!(store.subjects().unwrap().is_empty());
        assert!(store.latest("pets").unwrap().is_none());
        let admit = |text: &'static str| {
            move |history: &[Record]| -> Result<Admission, String> {
                if text == "refuse" {
                    return Err(format!("refused after {}", history.len()));
                }
                Ok(Admission {
                    bundle: bundle(text),
                    api_version: Some("1.0.0".into()),
                })
            }
        };
        let first = store
            .register("pets", "2026-09-28", admit("one"))
            .unwrap()
            .unwrap();
        assert_eq!((first.record.version, first.created), (1, true));
        let same = store
            .register("pets", "2026-09-29", admit("one"))
            .unwrap()
            .unwrap();
        assert_eq!((same.record.version, same.created), (1, false));
        let refused = store
            .register("pets", "2026-09-29", admit("refuse"))
            .unwrap();
        assert_eq!(refused.unwrap_err(), "refused after 1");
        let second = store
            .register("pets", "2026-09-29", admit("two"))
            .unwrap()
            .unwrap();
        assert_eq!(second.record.version, 2);
        assert_eq!(store.versions("pets").unwrap(), [1, 2]);
        assert_eq!(store.get("pets", 1).unwrap(), first.record);
        assert_eq!(store.latest("pets").unwrap().unwrap(), second.record);
        assert!(matches!(store.get("pets", 3), Err(StoreError::NotFound(_))));
        assert!(matches!(
            store.register("../x", "", admit("one")),
            Err(StoreError::Invalid(_))
        ));
        assert!(matches!(
            store.register("PETS", "", admit("one")),
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(store.subjects().unwrap(), ["pets"]);
    }

    #[test]
    fn compatibility_settings() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let default = Compatibility::default();
        assert_eq!(store.compatibility("pets", default).unwrap(), default);
        let strict = Compatibility::from_json(&json!({"transitive": true}), default).unwrap();
        assert_eq!(strict.fail_on, FailOn::Breaking);
        assert!(strict.transitive);
        store.set_compatibility("pets", strict).unwrap();
        assert_eq!(store.compatibility("pets", default).unwrap(), strict);
        assert!(Compatibility::from_json(&json!({"fail_on": "all"}), default).is_err());
        assert!(Compatibility::from_json(&json!({"mode": "full"}), default).is_err());
        // Settings alone do not make a subject.
        assert!(store.subjects().unwrap().is_empty());
    }
}
