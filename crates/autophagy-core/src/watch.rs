//! Continuous ingestion loop shared by the CLI watch command and daemon.
//!
//! The loop is model-free and ingest-only: it periodically drives a set of
//! incremental import [`WatchSource`]s against one open [`EventStore`] and never
//! executes, installs, or otherwise acts on the imported evidence. Adapters plug
//! in through the [`WatchSource`] trait, so `autophagy-core` stays decoupled from
//! the concrete native adapters (which live downstream of the store).

use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use autophagy_store::EventStore;
use serde::Serialize;

/// Error surfaced by a single source cycle. Kept as a boxed trait object so the
/// core loop never has to name adapter-specific error types.
pub type SourceError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// Normalized per-adapter result of one incremental import cycle.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct CycleOutcome {
    /// Adapter identity, for summary lines.
    pub adapter: String,
    /// Newly persisted canonical events.
    pub inserted: u64,
    /// Identical existing events that caused no writes.
    pub duplicates: u64,
    /// Valid events excluded by selection.
    pub skipped: u64,
    /// Same-ID/different-content events quarantined.
    pub conflicts: u64,
    /// Invalid or store-rejected records.
    pub rejected: u64,
    /// Retained source-addressed diagnostics.
    pub diagnostics: u64,
}

impl CycleOutcome {
    /// Start a zeroed outcome for one adapter.
    #[must_use]
    pub fn new(adapter: impl Into<String>) -> Self {
        Self {
            adapter: adapter.into(),
            ..Self::default()
        }
    }

    /// Whether this cycle produced nothing worth reporting: no new events, no
    /// conflicts, and no rejections. Duplicates and skips alone are quiet.
    #[must_use]
    pub const fn is_quiet(&self) -> bool {
        self.inserted == 0 && self.conflicts == 0 && self.rejected == 0
    }
}

/// One incremental import source driven once per cycle.
///
/// Implementations own their adapter options and translate the adapter's own
/// summary into a [`CycleOutcome`]. Repeat cycles are cheap and idempotent
/// because the store's source cursors skip already-imported bytes.
pub trait WatchSource {
    /// Stable adapter identifier used in summary lines.
    fn adapter(&self) -> &str;

    /// Run exactly one incremental import cycle against the open store.
    ///
    /// # Errors
    /// Returns a [`SourceError`] when discovery or import fails. A failure is
    /// per-cycle and non-fatal: the loop logs it and retries next cycle.
    fn import_cycle(&mut self, store: &mut EventStore) -> Result<CycleOutcome, SourceError>;
}

/// Loop timing and lifetime configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WatchConfig {
    /// Delay between the end of one cycle and the start of the next.
    pub interval: Duration,
    /// Run a single cycle and return instead of looping.
    pub once: bool,
}

/// Result of one adapter within a cycle.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AdapterOutcome {
    /// The adapter completed its cycle.
    Imported(CycleOutcome),
    /// The adapter failed; the loop continues.
    Failed(AdapterFailure),
}

/// A non-fatal, per-cycle adapter failure.
#[derive(Clone, Debug, Serialize)]
pub struct AdapterFailure {
    /// Adapter identity.
    pub adapter: String,
    /// Human-readable failure detail.
    pub message: String,
    /// Whether this failure is identical to the previous cycle's failure for
    /// the same adapter and was therefore deduplicated (kept out of the log to
    /// avoid spamming a persistent error).
    pub suppressed: bool,
}

/// Everything one cycle produced across all adapters.
#[derive(Clone, Debug, Serialize)]
pub struct CycleReport {
    /// One-based cycle counter for the lifetime of this loop.
    pub cycle: u64,
    /// Per-adapter outcomes in source order.
    pub outcomes: Vec<AdapterOutcome>,
}

impl CycleReport {
    /// Whether the whole cycle is quiet: every adapter either imported nothing
    /// notable or repeated an already-reported failure.
    #[must_use]
    pub fn is_quiet(&self) -> bool {
        self.outcomes.iter().all(|outcome| match outcome {
            AdapterOutcome::Imported(cycle) => cycle.is_quiet(),
            AdapterOutcome::Failed(failure) => failure.suppressed,
        })
    }
}

/// Aggregate counters returned when the loop exits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
pub struct WatchSummary {
    /// Cycles executed.
    pub cycles: u64,
    /// Total events inserted across all cycles.
    pub inserted: u64,
    /// Newly-reported per-adapter failures (excludes suppressed repeats of an
    /// identical error).
    pub failures: u64,
}

/// Drive incremental ingestion until shutdown is requested.
///
/// The `observer` is invoked with each [`CycleReport`] after the cycle
/// completes. `shutdown` is polled between adapters and during the inter-cycle
/// sleep, so a requested shutdown never interrupts an in-flight adapter import
/// (and therefore never interrupts an in-flight store transaction).
pub fn run_watch(
    store: &mut EventStore,
    sources: &mut [Box<dyn WatchSource>],
    config: &WatchConfig,
    shutdown: &Arc<AtomicBool>,
    observer: &mut dyn FnMut(&CycleReport),
) -> WatchSummary {
    let mut last_errors: HashMap<String, String> = HashMap::new();
    let mut summary = WatchSummary::default();
    let mut cycle = 0_u64;

    while !shutdown.load(Ordering::Relaxed) {
        cycle += 1;
        let report = run_cycle(store, sources, cycle, shutdown, &mut last_errors);
        for outcome in &report.outcomes {
            match outcome {
                AdapterOutcome::Imported(cycle) => summary.inserted += cycle.inserted,
                // Only count newly-reported failures. Suppressed repeats of an
                // identical error must not grow this counter without bound on a
                // long-running daemon.
                AdapterOutcome::Failed(failure) if !failure.suppressed => summary.failures += 1,
                AdapterOutcome::Failed(_) => {}
            }
        }
        summary.cycles += 1;
        observer(&report);

        if config.once {
            break;
        }
        if !sleep_interruptible(config.interval, shutdown) {
            break;
        }
    }

    summary
}

fn run_cycle(
    store: &mut EventStore,
    sources: &mut [Box<dyn WatchSource>],
    cycle: u64,
    shutdown: &Arc<AtomicBool>,
    last_errors: &mut HashMap<String, String>,
) -> CycleReport {
    let mut outcomes = Vec::with_capacity(sources.len());
    for source in &mut *sources {
        // Do not start a new adapter once shutdown is requested; the current
        // in-flight import (if any) always runs to completion.
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        let adapter = source.adapter().to_owned();
        match source.import_cycle(store) {
            Ok(outcome) => {
                last_errors.remove(&adapter);
                outcomes.push(AdapterOutcome::Imported(outcome));
            }
            Err(error) => {
                let message = error.to_string();
                let suppressed = last_errors.get(&adapter) == Some(&message);
                last_errors.insert(adapter.clone(), message.clone());
                outcomes.push(AdapterOutcome::Failed(AdapterFailure {
                    adapter,
                    message,
                    suppressed,
                }));
            }
        }
    }
    CycleReport { cycle, outcomes }
}

/// Sleep for `interval`, waking promptly if shutdown is requested. Returns
/// `false` when shutdown was observed (so the caller stops looping).
fn sleep_interruptible(interval: Duration, shutdown: &Arc<AtomicBool>) -> bool {
    const SLICE: Duration = Duration::from_millis(200);
    let mut remaining = interval;
    while remaining > Duration::ZERO {
        if shutdown.load(Ordering::Relaxed) {
            return false;
        }
        let nap = remaining.min(SLICE);
        thread::sleep(nap);
        remaining = remaining.saturating_sub(nap);
    }
    !shutdown.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A source that inserts a fixed count on its first cycle and nothing after,
    /// mirroring the incremental-cursor behaviour of a real adapter.
    struct CountingSource {
        adapter: String,
        remaining: u64,
    }

    impl WatchSource for CountingSource {
        fn adapter(&self) -> &str {
            &self.adapter
        }

        fn import_cycle(&mut self, _store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
            let inserted = self.remaining;
            self.remaining = 0;
            Ok(CycleOutcome {
                adapter: self.adapter.clone(),
                inserted,
                ..CycleOutcome::default()
            })
        }
    }

    /// A source that always fails with the same message.
    struct FailingSource {
        adapter: String,
    }

    impl WatchSource for FailingSource {
        fn adapter(&self) -> &str {
            &self.adapter
        }

        fn import_cycle(&mut self, _store: &mut EventStore) -> Result<CycleOutcome, SourceError> {
            Err("boom".into())
        }
    }

    fn temp_store() -> (tempfile::TempDir, EventStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let store = EventStore::open(dir.path().join("watch.db")).expect("open store");
        (dir, store)
    }

    #[test]
    fn once_runs_a_single_cycle_and_exits() {
        let (_dir, mut store) = temp_store();
        let mut sources: Vec<Box<dyn WatchSource>> = vec![Box::new(CountingSource {
            adapter: "claude-code".to_owned(),
            remaining: 3,
        })];
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut reports = Vec::new();
        let summary = run_watch(
            &mut store,
            &mut sources,
            &WatchConfig {
                interval: Duration::from_secs(60),
                once: true,
            },
            &shutdown,
            &mut |report| reports.push(report.clone()),
        );
        assert_eq!(summary.cycles, 1);
        assert_eq!(summary.inserted, 3);
        assert_eq!(reports.len(), 1);
        assert!(!reports[0].is_quiet());
    }

    #[test]
    fn second_cycle_inserts_nothing_and_is_quiet() {
        let (_dir, mut store) = temp_store();
        let mut source = CountingSource {
            adapter: "codex".to_owned(),
            remaining: 5,
        };
        let first = source.import_cycle(&mut store).expect("first");
        let second = source.import_cycle(&mut store).expect("second");
        assert_eq!(first.inserted, 5);
        assert!(!first.is_quiet());
        assert_eq!(second.inserted, 0);
        assert!(second.is_quiet());
    }

    #[test]
    fn one_adapter_failure_does_not_abort_the_cycle() {
        let (_dir, mut store) = temp_store();
        let mut sources: Vec<Box<dyn WatchSource>> = vec![
            Box::new(FailingSource {
                adapter: "codex".to_owned(),
            }),
            Box::new(CountingSource {
                adapter: "claude-code".to_owned(),
                remaining: 2,
            }),
        ];
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut reports = Vec::new();
        let summary = run_watch(
            &mut store,
            &mut sources,
            &WatchConfig {
                interval: Duration::from_secs(60),
                once: true,
            },
            &shutdown,
            &mut |report| reports.push(report.clone()),
        );
        assert_eq!(summary.inserted, 2);
        assert_eq!(summary.failures, 1);
        let outcomes = &reports[0].outcomes;
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(outcomes[0], AdapterOutcome::Failed(_)));
        assert!(matches!(outcomes[1], AdapterOutcome::Imported(_)));
    }

    #[test]
    fn repeated_identical_failures_are_deduplicated() {
        let (_dir, mut store) = temp_store();
        let mut sources: Vec<Box<dyn WatchSource>> = vec![Box::new(FailingSource {
            adapter: "codex".to_owned(),
        })];
        let shutdown = Arc::new(AtomicBool::new(false));
        let mut last_errors = HashMap::new();

        let first = run_cycle(&mut store, &mut sources, 1, &shutdown, &mut last_errors);
        let second = run_cycle(&mut store, &mut sources, 2, &shutdown, &mut last_errors);

        let AdapterOutcome::Failed(first_failure) = &first.outcomes[0] else {
            panic!("expected failure");
        };
        let AdapterOutcome::Failed(second_failure) = &second.outcomes[0] else {
            panic!("expected failure");
        };
        assert!(!first_failure.suppressed);
        assert!(second_failure.suppressed);
        assert!(!first.is_quiet());
        assert!(second.is_quiet());
    }

    #[test]
    fn preset_shutdown_prevents_any_cycle() {
        let (_dir, mut store) = temp_store();
        let mut sources: Vec<Box<dyn WatchSource>> = vec![Box::new(CountingSource {
            adapter: "claude-code".to_owned(),
            remaining: 1,
        })];
        let shutdown = Arc::new(AtomicBool::new(true));
        let mut reports = Vec::new();
        let summary = run_watch(
            &mut store,
            &mut sources,
            &WatchConfig {
                interval: Duration::from_secs(60),
                once: false,
            },
            &shutdown,
            &mut |report| reports.push(report.clone()),
        );
        assert_eq!(summary.cycles, 0);
        assert!(reports.is_empty());
    }
}
