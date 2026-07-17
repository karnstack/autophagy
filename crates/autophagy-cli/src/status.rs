//! At-a-glance local state (`autophagy status`).
//!
//! One fast, read-only snapshot: where the database is and how big it is, how
//! much has been imported and how fresh each adapter's import is, whether the
//! search index and background daemon are in place, the detector thresholds in
//! effect, and how many findings and mutation candidates exist. Everything here
//! is a COUNT-style query plus one deterministic detection pass; it works
//! against an empty database and with no config file.

use std::{
    collections::BTreeMap,
    io::Write,
    path::{Path, PathBuf},
};

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
    /// Deterministic findings at the effective thresholds. `None` unless
    /// `--with-findings` was passed: computing it is a full detection pass, not
    /// a COUNT query, so it is opt-in to keep `status` fast on large stores.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub findings: Option<usize>,
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
    /// Indexed signatures minted under a superseded signature grammar. When
    /// non-zero, the index predates the current grammar and a
    /// `reindex --index-tool-input` re-mints every row (ADR 0014).
    pub stale_signatures: u64,
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
pub fn run(
    database: Option<PathBuf>,
    config: &Config,
    with_findings: bool,
) -> Result<StatusReport, CliError> {
    let db_path = resolve_database_path(database.clone())?;
    // Record whether the database pre-existed *before* opening it, since
    // `open_store` creates and migrates the file. Size is read afterwards so it
    // reflects the actual on-disk database rather than a missing file.
    let existed_before = db_path.exists();

    let store = open_store(&db_path)?;
    let size_bytes = on_disk_size(&db_path);
    let stats = store.stats()?;
    let signatures = store.signature_count()?;
    // Every indexed signature is an `operation/<version>|…` key. Rows that do not
    // carry the current grammar's prefix were minted under a superseded grammar
    // and no longer match freshly minted signatures (ADR 0014); `reindex` heals
    // them.
    let stale_prefix = format!(
        "operation/{}|",
        autophagy_events::signature::SIGNATURE_SPEC_VERSION
    );
    let stale_signatures = store.signatures_below_version(&stale_prefix)?;
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

    // Thresholds are cheap (config or defaults). Findings are opt-in: the first
    // pass at a given corpus and thresholds loads and deserializes every event
    // and runs a full detection pass — digest-cost on a large store — so
    // `status` stays fast by default and only pays that cost when
    // `--with-findings` is passed. The result is cached in the store, so
    // repeats at an unchanged corpus are instant.
    let detector = config.detector_config();
    let findings = if with_findings {
        Some(
            crate::detection::detect_cached(&store, None, detector, false)?
                .findings
                .len(),
        )
    } else {
        None
    };

    // Probe the daemon through the same lifecycle seam `daemon status` uses.
    let daemon_report = daemon::status(database)?;

    Ok(StatusReport {
        database: DatabaseStatus {
            path: db_path.display().to_string(),
            exists: existed_before,
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
            stale_signatures,
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

/// Width of the label column in the text rendering.
const LABEL: usize = 10;

/// Render a [`StatusReport`] as human-readable text: a fixed label column,
/// aligned adapter rows, grouped digits, and plain-language states.
///
/// # Errors
///
/// Returns [`std::io::Error`] when the writer fails.
#[allow(clippy::too_many_lines)]
pub fn write_text(report: &StatusReport, writer: &mut impl Write) -> std::io::Result<()> {
    let row = |label: &str, text: &str| format!("{label:<LABEL$} {text}");
    let cont = |text: &str| format!("{:<LABEL$} {text}", "");

    let db = &report.database;
    writeln!(writer, "{}", row("Database", &db.path))?;
    let size = db
        .size_bytes
        .map_or_else(|| "empty".to_owned(), format_bytes);
    let freshness = if db.exists {
        String::new()
    } else {
        " · created new database (nothing imported yet)".to_owned()
    };
    writeln!(
        writer,
        "{}",
        cont(&format!(
            "{size} · schema v{}{freshness}",
            db.schema_version
        ))
    )?;
    let conflicts_note = if db.conflicts > 0 {
        format!(" · {} quarantined conflicts", group(db.conflicts))
    } else {
        String::new()
    };
    let row_counts = format!(
        "{} events · {} sessions · {} artifacts{conflicts_note}",
        group(db.events),
        group(db.sessions),
        group(db.artifacts)
    );
    writeln!(writer, "{}", cont(&row_counts))?;
    writeln!(writer)?;

    let search = if report.index.tool_input_indexed {
        format!(
            "{} command signatures indexed · exact recall on",
            group(i64::try_from(report.index.signatures).unwrap_or(i64::MAX))
        )
    } else if report.database.events == 0 {
        // Nothing imported yet: reindex has nothing to rebuild, so point the new
        // user at the first step that actually populates the database.
        "commands not indexed — run `autophagy setup` to import your sessions".to_owned()
    } else {
        // Events exist but were imported without indexing; reindex is the right
        // and only way to make their commands searchable.
        "commands not indexed — run `autophagy reindex --index-tool-input`".to_owned()
    };
    writeln!(writer, "{}", row("Search", &search))?;
    if report.index.stale_signatures > 0 {
        writeln!(
            writer,
            "{}",
            cont(&format!(
                "{} signatures use an older grammar — run `autophagy reindex --index-tool-input` to re-mint and detect recurring shapes",
                group(i64::try_from(report.index.stale_signatures).unwrap_or(i64::MAX))
            ))
        )?;
    }
    writeln!(writer)?;

    if report.adapters.is_empty() {
        writeln!(writer, "{}", row("Agents", "none imported yet"))?;
    } else {
        let name_width = report
            .adapters
            .iter()
            .map(|adapter| adapter.adapter.len())
            .max()
            .unwrap_or(0);
        let sessions: Vec<String> = report
            .adapters
            .iter()
            .map(|adapter| group(adapter.sessions))
            .collect();
        let events: Vec<String> = report
            .adapters
            .iter()
            .map(|adapter| group(adapter.events))
            .collect();
        let sessions_width = sessions.iter().map(String::len).max().unwrap_or(0);
        let events_width = events.iter().map(String::len).max().unwrap_or(0);
        for (index, adapter) in report.adapters.iter().enumerate() {
            let freshness = adapter.last_import_age.as_deref().map_or_else(
                || {
                    adapter.last_event_at.as_deref().map_or_else(
                        || "never imported".to_owned(),
                        |at| format!("last event {at}"),
                    )
                },
                |age| format!("imported {age}"),
            );
            let line = format!(
                "{:<name_width$}   {:>sessions_width$} sessions   {:>events_width$} events   {freshness}",
                adapter.adapter, sessions[index], events[index]
            );
            let rendered = if index == 0 {
                row("Agents", &line)
            } else {
                cont(&line)
            };
            writeln!(writer, "{rendered}")?;
        }
    }
    writeln!(writer)?;

    let floor = if report.detector.min_support_ratio_bps == 0 {
        "noise floor off".to_owned()
    } else {
        format!("noise floor {} bps", report.detector.min_support_ratio_bps)
    };
    writeln!(
        writer,
        "{}",
        row(
            "Detector",
            &format!(
                "{}+ occurrences across {}+ sessions · {floor}",
                report.detector.min_occurrences, report.detector.min_sessions
            )
        )
    )?;
    match report.findings {
        Some(found) => writeln!(
            writer,
            "{}",
            row("Findings", &format!("{found} at current thresholds"))
        )?,
        None => writeln!(
            writer,
            "{}",
            row(
                "Findings",
                "not computed — run `autophagy status --with-findings`"
            )
        )?,
    }
    let lessons = if report.candidates.is_empty() {
        "none yet — run `autophagy mutations propose`".to_owned()
    } else {
        report
            .candidates
            .iter()
            .map(|(state, count)| match state.as_str() {
                "candidate" => format!("{count} awaiting review"),
                other => format!("{count} {other}"),
            })
            .collect::<Vec<_>>()
            .join(" · ")
    };
    writeln!(writer, "{}", row("Lessons", &lessons))?;
    writeln!(writer)?;

    let daemon = if report.daemon.supported {
        let interval = report.daemon.configured_interval_seconds;
        match (report.daemon.unit_present, report.daemon.job_loaded) {
            (true, Some(true)) => format!(
                "running · checks every {interval}s ({})",
                report.daemon.platform
            ),
            (true, _) => format!(
                "installed but not running ({}) · `autophagy daemon status` for details",
                report.daemon.platform
            ),
            (false, _) => {
                "not installed — `autophagy daemon install` enables background watching".to_owned()
            }
        }
    } else {
        "unsupported on this platform — use `autophagy watch` under your own supervisor".to_owned()
    };
    writeln!(writer, "{}", row("Daemon", &daemon))?;
    writeln!(
        writer,
        "{}",
        row(
            "Config",
            &format!(
                "{}{}",
                report.config_path,
                if report.config_present {
                    ""
                } else {
                    " (not present — using defaults)"
                }
            )
        )
    )?;
    Ok(())
}

/// Group an integer's digits with thousands separators (`69,400`).
fn group(value: i64) -> String {
    let digits = value.unsigned_abs().to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    if value < 0 {
        format!("-{grouped}")
    } else {
        grouped
    }
}

/// True on-disk footprint of the database: the main file plus its WAL and
/// shared-memory sidecars.
///
/// The store opens in WAL journal mode, so the pages written by the migrations
/// that `open_store` just ran live in `<db>-wal` until a checkpoint. Reading
/// only the main file's length right after opening therefore undercounts a
/// freshly created database — it reports the ~4 KiB header rather than the
/// migrated schema — which is exactly the number a new user sees and disbelieves.
/// Summing the sidecars makes the reported size match what the database occupies.
fn on_disk_size(db_path: &Path) -> Option<u64> {
    let mut total = std::fs::metadata(db_path).ok()?.len();
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = db_path.as_os_str().to_owned();
        sidecar.push(suffix);
        if let Ok(meta) = std::fs::metadata(PathBuf::from(sidecar)) {
            total += meta.len();
        }
    }
    Some(total)
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
