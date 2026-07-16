//! Incremental, schema-tolerant import of local Codex rollout transcripts.

mod discovery;
mod importer;
mod normalize;

pub use discovery::{
    CodexDiscoveryError, DiscoveredRollout, DiscoveryPlan, default_sessions_root, discover,
};
pub use importer::{
    CodexImportDiagnostic, CodexImportError, CodexImportOptions, CodexImportSummary, import_codex,
};

/// Stable adapter identifier written to AEP and source provenance.
pub const ADAPTER_NAME: &str = "codex";
