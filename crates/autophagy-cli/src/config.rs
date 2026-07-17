//! Persistent, versioned configuration (`autophagy config …`).
//!
//! Autophagy keeps a single TOML file next to the database, in the same
//! platform-local application-support directory the database resolves to
//! (`ProjectDirs` for `sh.autophagy.Autophagy`). The config location is fixed to
//! that directory and does **not** move with `--database`: keeping it stable and
//! predictable is worth more than following an ad-hoc database path.
//!
//! Precedence for every configurable knob is: built-in default < config file <
//! explicit CLI flag. Whether a flag was *explicitly* passed is decided by
//! clap's [`ValueSource`], never by comparing against a default value, so a flag
//! set to the same value as the default still wins over the config file.
//!
//! The file is forward-compatible: unknown sections and keys warn (to stderr)
//! rather than fail, a missing file is silent defaults, and only a genuinely
//! malformed file is an error. The config never stores secrets — provider API
//! key environment variable names live in the synthesis manifest, not here.

use std::{
    fmt::Write as _,
    fs,
    path::{Path, PathBuf},
};

use clap::parser::ValueSource;
use directories::ProjectDirs;
use serde::Serialize;

use crate::{CliError, watch::NativeAdapter};

/// Wire version of the config schema. Bumped only on a breaking layout change.
pub const CONFIG_VERSION: u32 = 1;

/// Environment override for the directory that holds `config.toml`.
///
/// Documented as a test and advanced hook: it relocates the config file (and
/// nothing else) so tests can isolate configuration without mutating process
/// environment for `ProjectDirs`.
pub const CONFIG_DIR_ENV: &str = "AUTOPHAGY_CONFIG_DIR";

/// Config file name inside the resolved config directory.
pub const CONFIG_FILE_NAME: &str = "config.toml";

// Built-in defaults. Single source of truth: clap's `default_value_t` and the
// detector/watch defaults all reference these, so `config list` and the
// commands never disagree about what "default" means.
/// Default minimum supporting events for detection.
pub const DEFAULT_MIN_OCCURRENCES: u32 = 3;
/// Default minimum distinct supporting sessions for detection.
pub const DEFAULT_MIN_SESSIONS: u32 = 2;
/// Default anti-noise support-share floor in basis points (disabled).
pub const DEFAULT_MIN_SUPPORT_RATIO_BPS: u16 = 0;
/// Default watch/daemon discovery interval in seconds.
pub const DEFAULT_INTERVAL_SECONDS: u64 = 60;
/// Default synthesis provider identifier.
pub const DEFAULT_SYNTHESIS_PROVIDER: &str = "deterministic";

/// Resolve the directory that holds the config file.
///
/// Honours [`CONFIG_DIR_ENV`] first; otherwise the platform-local
/// application-support directory, identical to where the default database
/// lives.
///
/// # Errors
///
/// Returns [`CliError::DataDirectoryUnavailable`] when no platform directory can
/// be determined and no override is set.
pub fn config_dir() -> Result<PathBuf, CliError> {
    if let Some(dir) = std::env::var_os(CONFIG_DIR_ENV).filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let project = ProjectDirs::from("sh", "autophagy", "Autophagy")
        .ok_or(CliError::DataDirectoryUnavailable)?;
    Ok(project.data_local_dir().to_path_buf())
}

/// Absolute path to `config.toml`.
///
/// # Errors
///
/// Returns [`CliError`] when the config directory cannot be resolved.
pub fn config_path() -> Result<PathBuf, CliError> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}

/// Type of a configurable value, driving parsing, validation, and display.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyType {
    Bool,
    U32,
    /// Basis points, constrained to `0..=10000`.
    U16Bps,
    U64,
    /// Comma-separated list of non-empty tokens.
    StringList,
    /// One of the known synthesis provider identifiers.
    Provider,
    /// Free-form filesystem path string.
    PathString,
}

/// One recognized dotted key and how to interpret its value.
struct KeySpec {
    /// Dotted name, e.g. `detect.min_occurrences`.
    dotted: &'static str,
    /// Owning `[section]`.
    section: &'static str,
    /// Leaf key within the section.
    leaf: &'static str,
    /// Value type.
    ty: KeyType,
}

const KEYS: &[KeySpec] = &[
    KeySpec {
        dotted: "import.adapters",
        section: "import",
        leaf: "adapters",
        ty: KeyType::StringList,
    },
    KeySpec {
        dotted: "import.index_tool_input",
        section: "import",
        leaf: "index_tool_input",
        ty: KeyType::Bool,
    },
    KeySpec {
        dotted: "import.include_content",
        section: "import",
        leaf: "include_content",
        ty: KeyType::Bool,
    },
    KeySpec {
        dotted: "import.index_metadata",
        section: "import",
        leaf: "index_metadata",
        ty: KeyType::StringList,
    },
    KeySpec {
        dotted: "import.exclude_paths",
        section: "import",
        leaf: "exclude_paths",
        ty: KeyType::StringList,
    },
    KeySpec {
        dotted: "detect.min_occurrences",
        section: "detect",
        leaf: "min_occurrences",
        ty: KeyType::U32,
    },
    KeySpec {
        dotted: "detect.min_sessions",
        section: "detect",
        leaf: "min_sessions",
        ty: KeyType::U32,
    },
    KeySpec {
        dotted: "detect.min_support_ratio_bps",
        section: "detect",
        leaf: "min_support_ratio_bps",
        ty: KeyType::U16Bps,
    },
    KeySpec {
        dotted: "watch.interval_seconds",
        section: "watch",
        leaf: "interval_seconds",
        ty: KeyType::U64,
    },
    KeySpec {
        dotted: "synthesis.provider",
        section: "synthesis",
        leaf: "provider",
        ty: KeyType::Provider,
    },
    KeySpec {
        dotted: "synthesis.manifest_path",
        section: "synthesis",
        leaf: "manifest_path",
        ty: KeyType::PathString,
    },
];

/// Known top-level sections (plus the bare `config_version` header key).
const SECTIONS: &[&str] = &["import", "detect", "watch", "synthesis"];

const PROVIDERS: &[&str] = &[
    "deterministic",
    "ollama",
    "openai-compatible",
    "claude-cli",
    "codex-cli",
];

fn spec(dotted: &str) -> Option<&'static KeySpec> {
    KEYS.iter().find(|spec| spec.dotted == dotted)
}

/// Effective, typed configuration resolved from the file (defaults applied by
/// callers via the `*_or` accessors, so absent keys stay `None`).
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub import_adapters: Option<Vec<String>>,
    pub import_index_tool_input: Option<bool>,
    pub import_include_content: Option<bool>,
    pub import_index_metadata: Option<Vec<String>>,
    pub import_exclude_paths: Option<Vec<String>>,
    pub detect_min_occurrences: Option<u32>,
    pub detect_min_sessions: Option<u32>,
    pub detect_min_support_ratio_bps: Option<u16>,
    pub watch_interval_seconds: Option<u64>,
    pub synthesis_provider: Option<String>,
    pub synthesis_manifest_path: Option<String>,
    /// Whether a config file was actually present on disk.
    pub present: bool,
}

impl Config {
    /// Load configuration from the resolved path.
    ///
    /// A missing file yields silent defaults. Unknown sections and keys, and a
    /// `config_version` newer than this binary understands, are pushed onto
    /// `warnings` (the caller prints them to stderr) rather than failing.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Config`] for a malformed file (syntax) or a known key
    /// whose value has the wrong type or is out of range.
    pub fn load(warnings: &mut Vec<String>) -> Result<Self, CliError> {
        let path = config_path()?;
        Self::load_from(&path, warnings)
    }

    /// Load from an explicit path (the test/advanced seam).
    ///
    /// # Errors
    ///
    /// See [`Config::load`].
    pub fn load_from(path: &Path, warnings: &mut Vec<String>) -> Result<Self, CliError> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(error) => return Err(CliError::from(error)),
        };
        let table: toml::Table = text.parse().map_err(|error: toml::de::Error| {
            CliError::Config(format!(
                "{} is malformed: {}",
                path.display(),
                error.message()
            ))
        })?;
        Self::from_table(&table, warnings)
    }

    fn from_table(table: &toml::Table, warnings: &mut Vec<String>) -> Result<Self, CliError> {
        // Version header (forward-compat warning only).
        if let Some(version) = table.get("config_version") {
            match version.as_integer() {
                Some(found) if found > i64::from(CONFIG_VERSION) => warnings.push(format!(
                    "config_version {found} is newer than this build understands ({CONFIG_VERSION}); \
                     unknown keys are ignored"
                )),
                Some(_) => {}
                None => {
                    return Err(CliError::Config(
                        "config_version must be an integer".to_owned(),
                    ));
                }
            }
        }

        // Warn on unknown top-level keys/sections.
        for key in table.keys() {
            if key != "config_version" && !SECTIONS.contains(&key.as_str()) {
                warnings.push(format!("ignoring unknown config section `{key}`"));
            }
        }
        // Warn on unknown keys within known sections.
        for section in SECTIONS {
            if let Some(toml::Value::Table(entries)) = table.get(*section) {
                for leaf in entries.keys() {
                    let dotted = format!("{section}.{leaf}");
                    if spec(&dotted).is_none() {
                        warnings.push(format!("ignoring unknown config key `{dotted}`"));
                    }
                }
            }
        }

        let mut config = Self {
            present: true,
            ..Self::default()
        };
        config.import_adapters = read_string_list(table, "import.adapters")?;
        config.import_index_tool_input = read_bool(table, "import.index_tool_input")?;
        config.import_include_content = read_bool(table, "import.include_content")?;
        config.import_index_metadata = read_string_list(table, "import.index_metadata")?;
        config.import_exclude_paths = read_string_list(table, "import.exclude_paths")?;
        config.detect_min_occurrences = read_u32(table, "detect.min_occurrences")?;
        config.detect_min_sessions = read_u32(table, "detect.min_sessions")?;
        config.detect_min_support_ratio_bps = read_bps(table, "detect.min_support_ratio_bps")?;
        config.watch_interval_seconds = read_u64(table, "watch.interval_seconds")?;
        config.synthesis_provider = read_provider(table, "synthesis.provider")?;
        config.synthesis_manifest_path = read_path(table, "synthesis.manifest_path")?;
        Ok(config)
    }

    /// Detector configuration with built-in defaults applied where unset.
    #[must_use]
    pub fn detector_config(&self) -> autophagy_patterns::DetectorConfig {
        autophagy_patterns::DetectorConfig {
            min_occurrences: self
                .detect_min_occurrences
                .unwrap_or(DEFAULT_MIN_OCCURRENCES),
            min_sessions: self.detect_min_sessions.unwrap_or(DEFAULT_MIN_SESSIONS),
            min_support_ratio_bps: self
                .detect_min_support_ratio_bps
                .unwrap_or(DEFAULT_MIN_SUPPORT_RATIO_BPS),
        }
    }

    /// Watch/daemon interval with the built-in default applied where unset.
    #[must_use]
    pub const fn interval_or_default(&self) -> u64 {
        match self.watch_interval_seconds {
            Some(value) => value,
            None => DEFAULT_INTERVAL_SECONDS,
        }
    }

    /// Adapter set as `NativeAdapter` values, if configured and all valid.
    ///
    /// # Errors
    ///
    /// Returns [`CliError::Config`] when a configured adapter name is unknown.
    pub fn import_adapters_parsed(&self) -> Result<Option<Vec<NativeAdapter>>, CliError> {
        let Some(names) = &self.import_adapters else {
            return Ok(None);
        };
        let mut parsed = Vec::with_capacity(names.len());
        for name in names {
            parsed.push(parse_adapter(name)?);
        }
        Ok(Some(parsed))
    }
}

fn nested<'a>(table: &'a toml::Table, dotted: &str) -> Option<&'a toml::Value> {
    let (section, leaf) = dotted.split_once('.')?;
    match table.get(section)? {
        toml::Value::Table(entries) => entries.get(leaf),
        _ => None,
    }
}

fn type_error(dotted: &str, expected: &str) -> CliError {
    CliError::Config(format!("config key `{dotted}` must be {expected}"))
}

fn read_bool(table: &toml::Table, dotted: &str) -> Result<Option<bool>, CliError> {
    match nested(table, dotted) {
        None => Ok(None),
        Some(value) => value
            .as_bool()
            .map(Some)
            .ok_or_else(|| type_error(dotted, "a boolean")),
    }
}

fn read_u32(table: &toml::Table, dotted: &str) -> Result<Option<u32>, CliError> {
    // Both u32 keys (`detect.min_occurrences`, `detect.min_sessions`) are
    // recurrence gates, so zero is meaningless and rejected.
    match nested(table, dotted) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .and_then(|integer| u32::try_from(integer).ok())
            .filter(|count| *count >= 1)
            .map(Some)
            .ok_or_else(|| type_error(dotted, "a positive integer (>= 1)")),
    }
}

fn read_u64(table: &toml::Table, dotted: &str) -> Result<Option<u64>, CliError> {
    match nested(table, dotted) {
        None => Ok(None),
        Some(value) => value
            .as_integer()
            .and_then(|integer| u64::try_from(integer).ok())
            .map(Some)
            .ok_or_else(|| type_error(dotted, "a non-negative integer")),
    }
}

fn read_bps(table: &toml::Table, dotted: &str) -> Result<Option<u16>, CliError> {
    match nested(table, dotted) {
        None => Ok(None),
        Some(value) => {
            let integer = value
                .as_integer()
                .ok_or_else(|| type_error(dotted, "an integer in 0..=10000"))?;
            if (0..=10_000).contains(&integer) {
                Ok(Some(
                    u16::try_from(integer).expect("bounded above by 10000"),
                ))
            } else {
                Err(type_error(dotted, "an integer in 0..=10000"))
            }
        }
    }
}

fn read_path(table: &toml::Table, dotted: &str) -> Result<Option<String>, CliError> {
    match nested(table, dotted) {
        None => Ok(None),
        Some(value) => value
            .as_str()
            .map(|text| Some(text.to_owned()))
            .ok_or_else(|| type_error(dotted, "a string path")),
    }
}

fn read_provider(table: &toml::Table, dotted: &str) -> Result<Option<String>, CliError> {
    match read_path(table, dotted)? {
        None => Ok(None),
        Some(provider) if PROVIDERS.contains(&provider.as_str()) => Ok(Some(provider)),
        Some(other) => Err(CliError::Config(format!(
            "config key `{dotted}` must be one of {}; got `{other}`",
            PROVIDERS.join(", ")
        ))),
    }
}

fn read_string_list(table: &toml::Table, dotted: &str) -> Result<Option<Vec<String>>, CliError> {
    match nested(table, dotted) {
        None => Ok(None),
        Some(toml::Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                let text = item
                    .as_str()
                    .ok_or_else(|| type_error(dotted, "an array of strings"))?;
                out.push(text.to_owned());
            }
            Ok(Some(out))
        }
        Some(_) => Err(type_error(dotted, "an array of strings")),
    }
}

fn parse_adapter(name: &str) -> Result<NativeAdapter, CliError> {
    NativeAdapter::ALL
        .iter()
        .copied()
        .find(|adapter| adapter.as_str() == name)
        .ok_or_else(|| {
            CliError::Config(format!(
                "unknown adapter `{name}`; expected one of {}",
                NativeAdapter::ALL
                    .iter()
                    .map(|adapter| adapter.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })
}

// --- Precedence resolution --------------------------------------------------

/// Whether an argument was set on the command line (not defaulted).
#[must_use]
pub fn flag_set(matches: &clap::ArgMatches, id: &str) -> bool {
    matches.value_source(id) == Some(ValueSource::CommandLine)
}

/// Resolve a scalar under default < config < explicit flag.
fn resolve<T: Copy>(flag_present: bool, flag_value: T, configured: Option<T>, default: T) -> T {
    if flag_present {
        flag_value
    } else {
        configured.unwrap_or(default)
    }
}

/// Resolve a boolean gate (`--index-tool-input` and friends).
#[must_use]
pub fn resolve_bool(
    matches: &clap::ArgMatches,
    id: &str,
    flag_value: bool,
    configured: Option<bool>,
) -> bool {
    resolve(flag_set(matches, id), flag_value, configured, false)
}

/// Resolve a list argument: an explicit flag replaces the config list entirely.
#[must_use]
pub fn resolve_list(
    matches: &clap::ArgMatches,
    id: &str,
    flag_value: &[String],
    configured: Option<&[String]>,
) -> Vec<String> {
    if flag_set(matches, id) {
        flag_value.to_vec()
    } else {
        configured.map(<[String]>::to_vec).unwrap_or_default()
    }
}

/// Resolve the detector configuration for a threshold-bearing command.
#[must_use]
pub fn resolve_thresholds(
    matches: &clap::ArgMatches,
    args: crate::ThresholdArgs,
    config: &Config,
) -> autophagy_patterns::DetectorConfig {
    autophagy_patterns::DetectorConfig {
        min_occurrences: resolve(
            flag_set(matches, "min_occurrences"),
            args.min_occurrences,
            config.detect_min_occurrences,
            DEFAULT_MIN_OCCURRENCES,
        ),
        min_sessions: resolve(
            flag_set(matches, "min_sessions"),
            args.min_sessions,
            config.detect_min_sessions,
            DEFAULT_MIN_SESSIONS,
        ),
        min_support_ratio_bps: resolve(
            flag_set(matches, "min_support_ratio_bps"),
            args.min_support_ratio_bps,
            config.detect_min_support_ratio_bps,
            DEFAULT_MIN_SUPPORT_RATIO_BPS,
        ),
    }
}

/// Resolve the interval (seconds) for watch/daemon under precedence.
#[must_use]
pub fn resolve_interval(matches: &clap::ArgMatches, flag_value: u64, config: &Config) -> u64 {
    resolve(
        flag_set(matches, "interval"),
        flag_value,
        config.watch_interval_seconds,
        DEFAULT_INTERVAL_SECONDS,
    )
}

/// Resolve the native adapter selection under precedence, returning the
/// clap-parsed flag list unchanged when adapters were passed, else the
/// configured set, else empty (which every caller treats as "all").
///
/// # Errors
///
/// Returns [`CliError::Config`] when a configured adapter name is unknown.
pub fn resolve_adapters(
    matches: &clap::ArgMatches,
    flag_value: &[NativeAdapter],
    config: &Config,
) -> Result<Vec<NativeAdapter>, CliError> {
    if flag_set(matches, "adapters") {
        return Ok(flag_value.to_vec());
    }
    Ok(config.import_adapters_parsed()?.unwrap_or_default())
}

// --- `config` subcommand ----------------------------------------------------

/// One row of `config list`.
#[derive(Clone, Debug, Serialize)]
pub struct ConfigEntry {
    /// Dotted key name.
    pub key: String,
    /// Effective value rendered as a string (defaults applied).
    pub value: String,
    /// `"default"` or `"config"`.
    pub source: &'static str,
}

/// Structured result of a `config` subcommand.
#[derive(Debug, Serialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ConfigReport {
    List {
        path: String,
        present: bool,
        entries: Vec<ConfigEntry>,
    },
    Get {
        key: String,
        value: Option<String>,
        source: &'static str,
    },
    Set {
        key: String,
        value: String,
        path: String,
    },
    Unset {
        key: String,
        removed: bool,
        path: String,
    },
    Path {
        path: String,
    },
    Edit {
        path: String,
        edited: bool,
    },
}

/// Effective value + source for a single dotted key, given a loaded config.
fn effective(config: &Config, dotted: &str) -> (String, &'static str) {
    let (from_config, default): (Option<String>, String) = match dotted {
        "import.adapters" => (
            config.import_adapters.as_ref().map(|list| list.join(",")),
            NativeAdapter::ALL
                .iter()
                .map(|adapter| adapter.as_str())
                .collect::<Vec<_>>()
                .join(","),
        ),
        "import.index_tool_input" => (
            config.import_index_tool_input.map(|b| b.to_string()),
            "false".to_owned(),
        ),
        "import.include_content" => (
            config.import_include_content.map(|b| b.to_string()),
            "false".to_owned(),
        ),
        "import.index_metadata" => (
            config
                .import_index_metadata
                .as_ref()
                .map(|list| list.join(",")),
            String::new(),
        ),
        "import.exclude_paths" => (
            config
                .import_exclude_paths
                .as_ref()
                .map(|list| list.join(",")),
            String::new(),
        ),
        "detect.min_occurrences" => (
            config.detect_min_occurrences.map(|v| v.to_string()),
            DEFAULT_MIN_OCCURRENCES.to_string(),
        ),
        "detect.min_sessions" => (
            config.detect_min_sessions.map(|v| v.to_string()),
            DEFAULT_MIN_SESSIONS.to_string(),
        ),
        "detect.min_support_ratio_bps" => (
            config.detect_min_support_ratio_bps.map(|v| v.to_string()),
            DEFAULT_MIN_SUPPORT_RATIO_BPS.to_string(),
        ),
        "watch.interval_seconds" => (
            config.watch_interval_seconds.map(|v| v.to_string()),
            DEFAULT_INTERVAL_SECONDS.to_string(),
        ),
        "synthesis.provider" => (
            config.synthesis_provider.clone(),
            DEFAULT_SYNTHESIS_PROVIDER.to_owned(),
        ),
        "synthesis.manifest_path" => (config.synthesis_manifest_path.clone(), String::new()),
        _ => (None, String::new()),
    };
    match from_config {
        Some(value) => (value, "config"),
        None => (default, "default"),
    }
}

/// Execute a `config` subcommand.
///
/// Loads the file only where needed (`list`/`get`), so `path` and `edit` keep
/// working even against a malformed file the user is trying to repair.
///
/// # Errors
///
/// Returns [`CliError`] for path resolution, malformed files, validation
/// failures, or filesystem errors during `set`/`unset`/`edit`.
pub fn run(action: crate::ConfigAction) -> Result<ConfigReport, CliError> {
    match action {
        crate::ConfigAction::List => {
            let mut load_warnings = Vec::new();
            let config = Config::load(&mut load_warnings)?;
            for warning in &load_warnings {
                eprintln!("warning: {warning}");
            }
            let entries = KEYS
                .iter()
                .map(|spec| {
                    let (value, source) = effective(&config, spec.dotted);
                    ConfigEntry {
                        key: spec.dotted.to_owned(),
                        value,
                        source,
                    }
                })
                .collect();
            Ok(ConfigReport::List {
                path: config_path()?.display().to_string(),
                present: config.present,
                entries,
            })
        }
        crate::ConfigAction::Get { key } => {
            if spec(&key).is_none() {
                return Err(unknown_key(&key));
            }
            let mut load_warnings = Vec::new();
            let config = Config::load(&mut load_warnings)?;
            for warning in &load_warnings {
                eprintln!("warning: {warning}");
            }
            let (value, source) = effective(&config, &key);
            Ok(ConfigReport::Get {
                key,
                value: Some(value),
                source,
            })
        }
        crate::ConfigAction::Set { key, value } => {
            let spec = spec(&key).ok_or_else(|| unknown_key(&key))?;
            let typed = parse_value(spec, &value)?;
            let path = config_path()?;
            let mut table = read_table(&path)?;
            set_nested(&mut table, spec.section, spec.leaf, typed);
            write_table(&path, &table)?;
            Ok(ConfigReport::Set {
                key,
                value,
                path: path.display().to_string(),
            })
        }
        crate::ConfigAction::Unset { key } => {
            let spec = spec(&key).ok_or_else(|| unknown_key(&key))?;
            let path = config_path()?;
            let mut table = read_table(&path)?;
            let removed = unset_nested(&mut table, spec.section, spec.leaf);
            write_table(&path, &table)?;
            Ok(ConfigReport::Unset {
                key,
                removed,
                path: path.display().to_string(),
            })
        }
        crate::ConfigAction::Path => Ok(ConfigReport::Path {
            path: config_path()?.display().to_string(),
        }),
        crate::ConfigAction::Edit => edit(),
    }
}

/// Values `setup` persists back to the config file on completion.
pub struct SetupValues {
    /// Selected adapter identifiers.
    pub adapters: Vec<String>,
    /// Whether tool input is indexed for exact recall.
    pub index_tool_input: bool,
    /// Whether prompt/response/tool-result text is persisted.
    pub include_content: bool,
    /// Already-redacted metadata keys to index.
    pub index_metadata: Vec<String>,
    /// Watch/daemon discovery interval in seconds.
    pub interval_seconds: u64,
}

/// Persist `setup`'s chosen values, preserving unknown keys already on disk.
///
/// # Errors
///
/// Returns [`CliError`] for path resolution, a malformed existing file, or a
/// filesystem write failure.
pub fn write_setup(values: &SetupValues) -> Result<PathBuf, CliError> {
    let path = config_path()?;
    let mut table = read_table(&path)?;
    let strings = |items: &[String]| {
        toml::Value::Array(
            items
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect::<Vec<_>>(),
        )
    };
    set_nested(&mut table, "import", "adapters", strings(&values.adapters));
    set_nested(
        &mut table,
        "import",
        "index_tool_input",
        toml::Value::Boolean(values.index_tool_input),
    );
    set_nested(
        &mut table,
        "import",
        "include_content",
        toml::Value::Boolean(values.include_content),
    );
    set_nested(
        &mut table,
        "import",
        "index_metadata",
        strings(&values.index_metadata),
    );
    set_nested(
        &mut table,
        "watch",
        "interval_seconds",
        toml::Value::Integer(i64::try_from(values.interval_seconds).unwrap_or(i64::MAX)),
    );
    write_table(&path, &table)?;
    Ok(path)
}

fn unknown_key(key: &str) -> CliError {
    CliError::Config(format!(
        "unknown config key `{key}`; known keys: {}",
        KEYS.iter()
            .map(|spec| spec.dotted)
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Validate and convert a `config set` value string to a typed TOML value.
fn parse_value(spec: &KeySpec, value: &str) -> Result<toml::Value, CliError> {
    match spec.ty {
        KeyType::Bool => match value {
            "true" => Ok(toml::Value::Boolean(true)),
            "false" => Ok(toml::Value::Boolean(false)),
            _ => Err(type_error(spec.dotted, "`true` or `false`")),
        },
        KeyType::U32 => value
            .parse::<u32>()
            .ok()
            .filter(|count| *count >= 1)
            .map(|v| toml::Value::Integer(i64::from(v)))
            .ok_or_else(|| type_error(spec.dotted, "a positive integer (>= 1)")),
        KeyType::U16Bps => {
            let parsed = value
                .parse::<u16>()
                .map_err(|_| type_error(spec.dotted, "an integer in 0..=10000"))?;
            if parsed <= 10_000 {
                Ok(toml::Value::Integer(i64::from(parsed)))
            } else {
                Err(type_error(spec.dotted, "an integer in 0..=10000"))
            }
        }
        KeyType::U64 => value
            .parse::<u64>()
            .ok()
            .and_then(|parsed| i64::try_from(parsed).ok())
            .map(toml::Value::Integer)
            .ok_or_else(|| type_error(spec.dotted, "a non-negative integer")),
        KeyType::PathString => Ok(toml::Value::String(value.to_owned())),
        KeyType::Provider => {
            if PROVIDERS.contains(&value) {
                Ok(toml::Value::String(value.to_owned()))
            } else {
                Err(CliError::Config(format!(
                    "config key `{}` must be one of {}; got `{value}`",
                    spec.dotted,
                    PROVIDERS.join(", ")
                )))
            }
        }
        KeyType::StringList => {
            let items: Vec<toml::Value> = value
                .split(',')
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(|token| toml::Value::String(token.to_owned()))
                .collect();
            // Adapter lists are validated against the known set.
            if spec.dotted == "import.adapters" {
                for item in &items {
                    if let Some(name) = item.as_str() {
                        parse_adapter(name)?;
                    }
                }
            }
            Ok(toml::Value::Array(items))
        }
    }
}

fn read_table(path: &Path) -> Result<toml::Table, CliError> {
    match fs::read_to_string(path) {
        Ok(text) => text.parse().map_err(|error: toml::de::Error| {
            CliError::Config(format!(
                "{} is malformed: {}",
                path.display(),
                error.message()
            ))
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(toml::Table::new()),
        Err(error) => Err(CliError::from(error)),
    }
}

fn write_table(path: &Path, table: &toml::Table) -> Result<(), CliError> {
    let mut stamped = table.clone();
    // Stamp our version, but never downgrade a file already written by a newer
    // build: an older binary editing one value must not rewrite a higher
    // config_version, which would break the forward compatibility the loader
    // promises for such files.
    let keep_existing = stamped
        .get("config_version")
        .and_then(toml::Value::as_integer)
        .is_some_and(|version| version >= i64::from(CONFIG_VERSION));
    if !keep_existing {
        stamped.insert(
            "config_version".to_owned(),
            toml::Value::Integer(i64::from(CONFIG_VERSION)),
        );
    }
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let rendered = toml::to_string_pretty(&stamped)
        .map_err(|error| CliError::Config(format!("could not serialize config: {error}")))?;
    fs::write(path, rendered)?;
    Ok(())
}

fn set_nested(table: &mut toml::Table, section: &str, leaf: &str, value: toml::Value) {
    let entry = table
        .entry(section.to_owned())
        .or_insert_with(|| toml::Value::Table(toml::Table::new()));
    if !entry.is_table() {
        *entry = toml::Value::Table(toml::Table::new());
    }
    if let toml::Value::Table(entries) = entry {
        entries.insert(leaf.to_owned(), value);
    }
}

fn unset_nested(table: &mut toml::Table, section: &str, leaf: &str) -> bool {
    let Some(toml::Value::Table(entries)) = table.get_mut(section) else {
        return false;
    };
    let removed = entries.remove(leaf).is_some();
    if entries.is_empty() {
        table.remove(section);
    }
    removed
}

/// `config edit`: open `$EDITOR`, then re-validate; on invalid result keep a
/// `.bak` backup and refuse to install the broken file.
fn edit() -> Result<ConfigReport, CliError> {
    let path = config_path()?;
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    if !path.exists() {
        // Seed a minimal, valid file so the editor opens on something.
        let mut seed = toml::Table::new();
        seed.insert(
            "config_version".to_owned(),
            toml::Value::Integer(i64::from(CONFIG_VERSION)),
        );
        write_table(&path, &seed)?;
    }
    let backup = path.with_extension("toml.bak");
    fs::copy(&path, &backup)?;

    let editor = std::env::var("VISUAL")
        .or_else(|_| std::env::var("EDITOR"))
        .unwrap_or_else(|_| "vi".to_owned());
    // Split the editor command so `EDITOR="code --wait"` works.
    let mut parts = editor.split_whitespace();
    let program = parts.next().unwrap_or("vi");
    let status = std::process::Command::new(program)
        .args(parts)
        .arg(&path)
        .status()
        .map_err(|error| {
            CliError::Config(format!("could not launch editor `{editor}`: {error}"))
        })?;
    if !status.success() {
        return Err(CliError::Config(format!(
            "editor `{editor}` exited without success; config left unchanged"
        )));
    }

    // Re-validate the edited file. On failure, restore from backup and error.
    let mut warnings = Vec::new();
    match Config::load_from(&path, &mut warnings) {
        Ok(_) => {
            let _ = fs::remove_file(&backup);
            for warning in &warnings {
                eprintln!("warning: {warning}");
            }
            Ok(ConfigReport::Edit {
                path: path.display().to_string(),
                edited: true,
            })
        }
        Err(error) => {
            // Keep the broken edit visible for the user to fix, but restore the
            // last-good config so the tool keeps working.
            let broken = path.with_extension("toml.invalid");
            let _ = fs::rename(&path, &broken);
            fs::rename(&backup, &path)?;
            Err(CliError::Config(format!(
                "{error}; your edit was saved to {} and the previous config was restored",
                broken.display()
            )))
        }
    }
}

/// Render a [`ConfigReport`] as human-readable text.
///
/// # Errors
///
/// Returns [`std::io::Error`] when the writer fails.
pub fn write_text(report: &ConfigReport, writer: &mut impl std::io::Write) -> std::io::Result<()> {
    match report {
        ConfigReport::List {
            path,
            present,
            entries,
        } => {
            writeln!(
                writer,
                "config: {path}{}",
                if *present {
                    ""
                } else {
                    " (not present — showing defaults)"
                }
            )?;
            let mut line = String::new();
            for entry in entries {
                line.clear();
                let _ = write!(line, "{}\t{}\t[{}]", entry.key, entry.value, entry.source);
                writeln!(writer, "{line}")?;
            }
            writeln!(
                writer,
                "explicit CLI flags override both config and defaults."
            )?;
        }
        ConfigReport::Get { key, value, source } => match value {
            Some(value) => writeln!(writer, "{key}\t{value}\t[{source}]")?,
            None => writeln!(writer, "{key}\t(unset)")?,
        },
        ConfigReport::Set { key, value, path } => {
            writeln!(writer, "set {key} = {value}")?;
            writeln!(writer, "wrote {path}")?;
        }
        ConfigReport::Unset { key, removed, path } => {
            writeln!(
                writer,
                "{} {key}",
                if *removed { "unset" } else { "already unset" }
            )?;
            writeln!(writer, "wrote {path}")?;
        }
        ConfigReport::Path { path } => writeln!(writer, "{path}")?,
        ConfigReport::Edit { path, edited } => {
            if *edited {
                writeln!(writer, "validated and saved {path}")?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(text: &str) -> (Config, Vec<String>) {
        let mut warnings = Vec::new();
        let parsed: toml::Table = text.parse().expect("valid toml");
        let config = Config::from_table(&parsed, &mut warnings).expect("valid config");
        (config, warnings)
    }

    #[test]
    fn missing_keys_stay_none() {
        let (config, warnings) = table("config_version = 1\n");
        assert!(config.detect_min_occurrences.is_none());
        assert!(config.import_index_tool_input.is_none());
        assert!(warnings.is_empty());
    }

    #[test]
    fn known_values_parse() {
        let (config, warnings) = table(
            "config_version = 1\n[detect]\nmin_occurrences = 5\n[import]\nindex_tool_input = true\nadapters = [\"claude-code\", \"codex\"]\n[watch]\ninterval_seconds = 120\n",
        );
        assert_eq!(config.detect_min_occurrences, Some(5));
        assert_eq!(config.import_index_tool_input, Some(true));
        assert_eq!(config.watch_interval_seconds, Some(120));
        assert_eq!(
            config.import_adapters.as_deref(),
            Some(["claude-code".to_owned(), "codex".to_owned()].as_slice())
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn unknown_keys_warn_without_failing() {
        let (_config, warnings) = table("[detect]\nmystery = 1\n[nonsense]\nx = 2\n");
        assert!(warnings.iter().any(|w| w.contains("detect.mystery")));
        assert!(warnings.iter().any(|w| w.contains("nonsense")));
    }

    #[test]
    fn wrong_type_is_a_precise_error() {
        let mut warnings = Vec::new();
        let parsed: toml::Table = "[detect]\nmin_occurrences = \"lots\"\n"
            .parse()
            .expect("valid toml");
        let error = Config::from_table(&parsed, &mut warnings).expect_err("type error");
        let message = error.to_string();
        assert!(message.contains("detect.min_occurrences"), "{message}");
    }

    #[test]
    fn bps_range_is_enforced() {
        let mut warnings = Vec::new();
        let parsed: toml::Table = "[detect]\nmin_support_ratio_bps = 20000\n"
            .parse()
            .expect("valid toml");
        assert!(Config::from_table(&parsed, &mut warnings).is_err());
    }

    #[test]
    fn newer_version_warns() {
        let (_config, warnings) = table("config_version = 999\n");
        assert!(warnings.iter().any(|w| w.contains("newer")));
    }

    #[test]
    fn set_then_read_round_trips_and_preserves_unknown_keys() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        // Seed an unknown key that a newer build might have written.
        fs::write(&path, "[future]\nknob = 7\n").expect("seed");

        let mut table = read_table(&path).expect("read");
        let spec = spec("detect.min_occurrences").expect("spec");
        let value = parse_value(spec, "9").expect("parse");
        set_nested(&mut table, spec.section, spec.leaf, value);
        write_table(&path, &table).expect("write");

        let mut warnings = Vec::new();
        let config = Config::load_from(&path, &mut warnings).expect("load");
        assert_eq!(config.detect_min_occurrences, Some(9));
        // The unknown section survived the write and still warns on load.
        assert!(warnings.iter().any(|w| w.contains("future")));
        let text = fs::read_to_string(&path).expect("read text");
        assert!(text.contains("config_version"));
        assert!(text.contains("[future]"));
    }

    #[test]
    fn unset_removes_key_and_empty_section() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "[detect]\nmin_sessions = 4\n").expect("seed");
        let mut table = read_table(&path).expect("read");
        assert!(unset_nested(&mut table, "detect", "min_sessions"));
        write_table(&path, &table).expect("write");
        let text = fs::read_to_string(&path).expect("read text");
        assert!(!text.contains("[detect]"), "empty section removed: {text}");
    }

    #[test]
    fn write_does_not_downgrade_a_newer_config_version() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        fs::write(&path, "config_version = 999\n[detect]\nmin_sessions = 3\n").expect("seed");
        let mut table = read_table(&path).expect("read");
        let key = spec("detect.min_sessions").expect("spec");
        set_nested(
            &mut table,
            key.section,
            key.leaf,
            parse_value(key, "4").expect("value"),
        );
        write_table(&path, &table).expect("write");
        let text = fs::read_to_string(&path).expect("read text");
        assert!(
            text.contains("config_version = 999"),
            "newer version preserved: {text}"
        );
    }

    #[test]
    fn zero_recurrence_gates_are_rejected_on_set_and_load() {
        assert!(parse_value(spec("detect.min_occurrences").expect("spec"), "0").is_err());
        assert!(parse_value(spec("detect.min_sessions").expect("spec"), "0").is_err());
        let mut warnings = Vec::new();
        let parsed: toml::Table = "[detect]\nmin_occurrences = 0\n"
            .parse()
            .expect("valid toml");
        assert!(Config::from_table(&parsed, &mut warnings).is_err());
    }

    #[test]
    fn set_rejects_invalid_values() {
        assert!(parse_value(spec("import.index_tool_input").expect("spec"), "maybe").is_err());
        assert!(parse_value(spec("synthesis.provider").expect("spec"), "gpt-9").is_err());
        assert!(
            parse_value(
                spec("import.adapters").expect("spec"),
                "claude-code,not-real"
            )
            .is_err()
        );
    }
}
