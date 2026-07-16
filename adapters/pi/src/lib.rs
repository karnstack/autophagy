//! Incremental, schema-tolerant import of local Pi coding-agent sessions.

mod discovery;
mod importer;
mod normalize;

pub use discovery::{
    DiscoveredSession, DiscoveryPlan, PiDiscoveryError, default_sessions_root, discover,
};
pub use importer::{
    PiImportDiagnostic, PiImportError, PiImportOptions, PiImportSummary, import_pi,
};

/// Stable adapter identifier written to AEP and source provenance.
pub const ADAPTER_NAME: &str = "pi";
