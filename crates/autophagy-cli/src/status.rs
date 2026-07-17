//! At-a-glance local state (`autophagy status`).
//!
//! One fast, read-only snapshot: where the database is and how big it is, how
//! much has been imported and how fresh each adapter's import is, whether the
//! search index and background daemon are in place, the detector thresholds in
//! effect, and how many findings and mutation candidates exist. Everything here
//! is a COUNT-style query plus one deterministic detection pass; it works
//! against an empty database and with no config file.

use std::{collections::BTreeMap, io::Write, path::PathBuf};

use autophagy_patterns::detect;
use serde::Serialize;
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::{CliError, config::Config, daemon, open_store, resolve_database_path};

/// Structured `status` result.
#[derive(Debug, Serialize)]
pub struct StatusReport {
    /// Database location and row counts.
    pub database: DatabaseStatus,
    /// Search index state.
    pub index: IndexStatus,
    /// Per-adapter import activity and freshness.
    pub adapters: Vec<AdapterStatus>,
    /// Detector thresholds in effect (config or built-in defaults).
    pub detector: DetectorStatus,
    /// Deterministic findings at the effective thresholds.
    pub findings: usize,
    /// Mutation candidates grouped by lifecycle state.
    pub candidates: BTreeMap<String, i64>,
    /// Background daemon state.
    pub daemon: DaemonStatus,
    /// Resolved config file path.
    pub config_path: String,
    /// Whether a config file is present on disk.
    pub config_present: bool,
}

/// Database location and row counts.
#[derive(Debug, Serialize)]
pub struct DatabaseStatus {
    pub path: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    pub schema_version: i64,
    pub sources: i64,
    pub sessions: i64,
    pub events: i64,
    pub artifacts: i64,
    pub conflicts: i64,
}

/// Search index state.
#[derive(Debug, Serialize)]
pub struct IndexStatus {
    /// Rows in the exact normalized-signature index.
    pub signatures: u64,
    /// Whether redacted tool input has been indexed for exact recall.
    pub tool_input_indexed: bool,
}

/// One adapter's import activity and freshness.
#[derive(Debug, Serialize)]
pub struct AdapterStatus {
    pub adapter: String,
    pub sessions: i64,
    pub events: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_event_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_import_at: Option<String>,
    /// Human-readable age of the last incremental import, when known.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_import_age: Option<String>,
}

/// Detector thresholds in effect.
#[allow(clippy::struct_field_names)]
#[derive(Debug, Serialize)]
pub struct DetectorStatus {
    pub min_occurrences: u32,
    pub min_sessions: u32,
    pub min_support_ratio_bps: u16,
}

/// Background daemon state.
#[derive(Debug, Serialize)]
pub struct DaemonStatus {
    pub platform: &'static str,
    pub supported: bool,
    pub unit_present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_loaded: Option<bool>,
    /// Configured discovery interval; the running unit uses whatever was baked
    /// in at `daemon install` time, so a config change needs a reinstall.
    pub configured_interval_seconds: u64,
}

/// Gather a status snapshot.
///
/// # Errors
///
/// Returns [`CliError`] for database, config, or daemon-probe failures.
pub fn run(database: Option<PathBuf>, config: &Config) -> Result<StatusReport, CliError> {
    let db_path = resolve_database_path(database.clone())?;
    let size_bytes = std::fs::metadata(&db_path).ok().map(|meta| meta.len());
    let exists = db_path.exists();

    let store = open_store(&db_path)?;
    let stats = store.stats()?;
    let signatures = store.signature_count()?;
    let activity = store.adapter_activity()?;
    let candidates = store.mutation_state_counts()?;
    let schema_version = store.schema_version()?;

    let now = OffsetDateTime::now_utc();
    let adapters = activity
        .into_iter()
        .map(|row| AdapterStatus {
            adapter: row.adapter,
            sessions: row.sessions,
            events: row.events,
            last_import_age: row
                .last_import_at
                .as_deref()
                .and_then(|at| humanize(at, now)),
            last_event_at: row.last_event_at,
            last_import_at: row.last_import_at,
        })
        .collect();

    // Findings at the effective (config or default) thresholds. This is the one
    // computed metric; every other field is a COUNT query.
    let detector = config.detector_config();
    let events = store.list_events_for_detection(None)?;
    let findings = detect(&events, detector).len();

    // Probe the daemon through the same lifecycle seam `daemon status` uses.
    let daemon_report = daemon::status(database)?;

    Ok(StatusReport {
        database: DatabaseStatus {
            path: db_path.display().to_string(),
            exists,
            size_bytes,
            schema_version,
            sources: stats.sources,
            sessions: stats.sessions,
            events: stats.events,
            artifacts: stats.artifacts,
            conflicts: stats.conflicts,
        },
        index: IndexStatus {
            signatures,
            tool_input_indexed: signatures > 0,
        },
        adapters,
        detector: DetectorStatus {
            min_occurrences: detector.min_occurrences,
            min_sessions: detector.min_sessions,
            min_support_ratio_bps: detector.min_support_ratio_bps,
        },
        findings,
        candidates,
        daemon: DaemonStatus {
            platform: daemon_report.platform,
            supported: daemon_report.supported,
            unit_present: daemon_report.unit_present,
            job_loaded: daemon_report.job_loaded,
            configured_interval_seconds: config.interval_or_default(),
        },
        config_path: crate::config::config_path()?.display().to_string(),
        config_present: config.present,
    })
}

/// Render an RFC3339 timestamp's age relative to `now` in coarse, human units.
fn humanize(timestamp: &str, now: OffsetDateTime) -> Option<String> {
    let then = OffsetDateTime::parse(timestamp, &Rfc3339).ok()?;
    let seconds = (now - then).whole_seconds();
    if seconds < 0 {
        return Some("in the future".to_owned());
    }
    let seconds = seconds.unsigned_abs();
    let text = match seconds {
        0..=59 => format!("{seconds}s ago"),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    };
    Some(text)
}

/// Render a [`StatusReport`] as human-readable text.
///
/// # Errors
///
/// Returns [`std::io::Error`] when the writer fails.
#[allow(clippy::too_many_lines)]
pub fn write_text(report: &StatusReport, writer: &mut impl Write) -> std::io::Result<()> {
    let db = &report.database;
    writeln!(writer, "database: {}", db.path)?;
    let size = db
        .size_bytes
        .map_or_else(|| "absent".to_owned(), format_bytes);
    writeln!(
        writer,
        "  {} · schema v{} · {}",
        size,
        db.schema_version,
        if db.exists { "present" } else { "new" }
    )?;
    writeln!(
        writer,
        "  {} events · {} sessions · {} sources · {} artifacts · {} conflicts",
        db.events, db.sessions, db.sources, db.artifacts, db.conflicts
    )?;

    writeln!(
        writer,
        "index: {} signatures · commands searchable: {}",
        report.index.signatures,
        if report.index.tool_input_indexed {
            "yes"
        } else {
            "no"
        }
    )?;

    if report.adapters.is_empty() {
        writeln!(writer, "adapters: none imported")?;
    } else {
        writeln!(writer, "adapters:")?;
        for adapter in &report.adapters {
            let freshness = adapter
                .last_import_age
                .as_deref()
                .or(adapter.last_event_at.as_deref())
                .unwrap_or("no imports");
            writeln!(
                writer,
                "  {}\t{} sessions · {} events · last import {}",
                adapter.adapter, adapter.sessions, adapter.events, freshness
            )?;
        }
    }

    writeln!(
        writer,
        "detector: min_occurrences={} · min_sessions={} · min_support_ratio_bps={}",
        report.detector.min_occurrences,
        report.detector.min_sessions,
        report.detector.min_support_ratio_bps
    )?;
    writeln!(writer, "findings: {}", report.findings)?;

    if report.candidates.is_empty() {
        writeln!(writer, "candidates: none")?;
    } else {
        let summary = report
            .candidates
            .iter()
            .map(|(state, count)| format!("{count} {state}"))
            .collect::<Vec<_>>()
            .join(" · ");
        writeln!(writer, "candidates: {summary}")?;
    }

    if report.daemon.supported {
        writeln!(
            writer,
            "daemon ({}): unit {} · loaded {} · configured interval {}s",
            report.daemon.platform,
            if report.daemon.unit_present {
                "present"
            } else {
                "absent"
            },
            describe(report.daemon.job_loaded),
            report.daemon.configured_interval_seconds
        )?;
    } else {
        writeln!(writer, "daemon: unsupported on this platform")?;
    }

    writeln!(
        writer,
        "config: {}{}",
        report.config_path,
        if report.config_present {
            ""
        } else {
            " (not present — using defaults)"
        }
    )?;
    Ok(())
}

fn describe(loaded: Option<bool>) -> &'static str {
    match loaded {
        Some(true) => "yes",
        Some(false) => "no",
        None => "unknown",
    }
}

fn format_bytes(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KiB", "MiB", "GiB"];
    #[allow(clippy::cast_precision_loss)]
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
