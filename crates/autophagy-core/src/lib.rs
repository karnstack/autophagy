//! Reusable application services for importing and digesting agent activity.

mod generic_jsonl;
mod watch;

pub use generic_jsonl::{
    ImportDiagnostic, ImportDiagnosticCode, ImportError, ImportOptions, ImportSummary, import_jsonl,
};
pub use watch::{
    AdapterFailure, AdapterOutcome, CycleOutcome, CycleReport, SourceError, WatchConfig,
    WatchSource, WatchSummary, run_watch,
};
