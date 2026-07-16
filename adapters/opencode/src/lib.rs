//! Incremental, schema-tolerant import of local `OpenCode` session storage.
//!
//! `OpenCode` persists each conversation as many small JSON files under a
//! `storage/` tree: one session info file, one file per message, and one file
//! per message part. This adapter treats a session as the incremental unit and
//! resumes past the highest message identifier it has already normalized.

mod discovery;
mod importer;
mod normalize;

pub use discovery::{
    DiscoveredSession, DiscoveryPlan, OpenCodeDiscoveryError, default_storage_root, discover,
};
pub use importer::{
    OpenCodeImportDiagnostic, OpenCodeImportError, OpenCodeImportOptions, OpenCodeImportSummary,
    import_opencode,
};

/// Stable adapter identifier written to AEP and source provenance.
pub const ADAPTER_NAME: &str = "opencode";
