//! Incremental, privacy-conscious import of Claude Code session transcripts.

mod discovery;
mod importer;
mod normalize;

pub use discovery::{
    DiscoveredSession, DiscoveryError, DiscoveryOptions, DiscoveryPlan, SessionKind,
    default_projects_root, discover,
};
pub use importer::{
    ClaudeImportDiagnostic, ClaudeImportError, ClaudeImportOptions, ClaudeImportSummary,
    import_claude_code,
};

/// Stable adapter identifier written to AEP envelopes and source provenance.
pub const ADAPTER_NAME: &str = "claude-code";
