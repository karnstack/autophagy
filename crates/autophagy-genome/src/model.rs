use autophagy_events::Event;
use autophagy_mutations::MutationPackage;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The genome bundle contract version this crate builds and parses.
pub const GENOME_SPEC_VERSION: &str = "genome/0.1";

/// Human-readable description of the redaction policy applied at export. The
/// exact path exclusions are the receiver-visible `import.exclude_paths`; the
/// secret rules are the built-in conservative set in `autophagy-redaction`.
pub const POLICY_DESCRIPTION: &str = "built-in secret rules + import.exclude_paths";

/// A self-contained, redaction-gated mutation genome.
///
/// One developer exports a verified candidate as this bundle; another imports it
/// as a fresh review-only candidate. The bundle carries the immutable mutation
/// package, the exact (redacted) AEP events its hypothesis and verification
/// reports cite — so the evidence foreign-key wall and content-hash-locked
/// reports stay valid on the receiver — the origin's verification reports as
/// display-only attestations, and the lifecycle transition history for context.
/// The candidate's lifecycle STATE never travels (see ADR 0016).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenomeBundle {
    /// Bundle contract version (`genome/0.1`).
    pub spec_version: String,
    /// Stable content identity: `gen_` + SHA-256 hex of the canonical bundle
    /// content with this field removed. Recomputed and checked on import.
    pub genome_id: String,
    /// RFC 3339 export timestamp.
    pub exported_at: String,
    /// Where this genome came from.
    pub origin: GenomeOrigin,
    /// The complete immutable mutation package, with reviewable text scrubbed.
    pub mutation: MutationPackage,
    /// The redacted AEP events cited by the hypothesis and the attestations, in
    /// stable event-id order.
    pub evidence_events: Vec<Event>,
    /// Origin-claimed, display-only verification reports.
    pub attestations: Vec<GenomeAttestation>,
    /// Origin lifecycle transition history, for context only.
    pub transitions: Vec<GenomeTransition>,
    /// What redaction did to this bundle.
    pub redaction: GenomeRedaction,
}

/// Stable provenance for an exported genome.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenomeOrigin {
    /// Stable identity of the exporting Autophagy instance.
    pub instance_key: String,
    /// The exporting binary's version string.
    pub autophagy_version: String,
}

/// A verification evaluation family that can be attested.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttestationKind {
    /// A deterministic replay evaluation.
    Replay,
    /// An observation-only shadow evaluation.
    Shadow,
}

impl AttestationKind {
    /// Stable wire and database representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Replay => "replay",
            Self::Shadow => "shadow",
        }
    }
}

/// One origin-claimed verification report carried by the bundle.
///
/// An attestation is a museum label. It records that the origin ran a replay or
/// shadow evaluation and what it claimed, so a reviewer can see the provenance;
/// it does NOT let the receiver skip local re-verification. The `content_hash`
/// is a transit-integrity fingerprint over `report_json`, checked on import.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenomeAttestation {
    /// Evaluation family.
    pub kind: AttestationKind,
    /// Stable report identity from the origin (`rep_…`/`shd_…`).
    pub id: String,
    /// Stable scenario/observation set hash the origin evaluated.
    pub set_hash: String,
    /// The complete versioned report exactly as the origin stored it.
    pub report_json: Value,
    /// SHA-256 hex of the report bytes, for the transit-integrity check.
    pub content_hash: String,
    /// Whether the origin claimed the evaluation passed.
    pub passed: bool,
    /// RFC 3339 timestamp the origin recorded the report.
    pub created_at: String,
}

impl GenomeAttestation {
    /// Whether `report_json` still hashes to the carried `content_hash`.
    ///
    /// This is a transit-integrity check only: a match proves the report bytes
    /// were not altered after export, never that the receiver reproduced the
    /// origin's result.
    #[must_use]
    pub fn hash_matches(&self) -> bool {
        crate::report_content_hash_hex(&self.report_json) == self.content_hash
    }
}

/// One origin lifecycle transition, carried for display only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenomeTransition {
    /// Previous state; absent for initial generation.
    pub from_state: Option<String>,
    /// New lifecycle state.
    pub to_state: String,
    /// Human-readable transition reason.
    pub reason: String,
    /// RFC 3339 transition timestamp.
    pub occurred_at: String,
}

/// A summary of what redaction did to the bundle at export.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenomeRedaction {
    /// Number of string fields the secret rules changed across events and the
    /// package text.
    pub redacted_fields: u64,
    /// Human-readable description of the policy applied.
    pub policy: String,
}
