//! Portable, redaction-gated mutation genome bundle format (`genome/0.1`).
//!
//! A genome is a single self-contained JSON file that carries one verified
//! mutation candidate from the machine that produced it to another. This crate
//! owns the wire format only: it builds a bundle from local materials (applying
//! the redaction gate), and parses and integrity-checks a bundle on the way in.
//! It performs no storage or network I/O — the CLI orchestrates gathering from
//! and ingesting into the store. See ADR 0016 and `docs/specs/genome/0.1`.
//!
//! Two invariants make the bundle safe to share and safe to trust:
//!
//! - **Redaction gate.** Every event runs through [`autophagy_redaction`]'s
//!   policy at export; a path-excluded event ABORTS the export (dropping it
//!   silently would break the content-hash-locked reports that cite it). The
//!   mutation's reviewable text is scrubbed with the same secret rules.
//! - **Trust is not transplanted.** The bundle carries verification reports as
//!   display-only attestations, never lifecycle state. The receiver re-verifies
//!   locally to advance the candidate.

mod model;

use autophagy_events::Event;
use autophagy_mutations::MutationPackage;
use autophagy_redaction::{PrivacyError, PrivacyPolicy};
use serde_json::Value;
use sha2::{Digest, Sha256};

pub use model::{
    AttestationKind, GENOME_SPEC_VERSION, GenomeAttestation, GenomeBundle, GenomeOrigin,
    GenomeRedaction, GenomeTransition, POLICY_DESCRIPTION,
};

/// Local materials for one genome, before redaction and assembly.
pub struct GenomeSource {
    /// Stable identity of the exporting instance.
    pub origin_instance_key: String,
    /// The exporting binary's version string.
    pub autophagy_version: String,
    /// RFC 3339 export timestamp.
    pub exported_at: String,
    /// The mutation package to export (reviewable text is scrubbed by `build`).
    pub package: MutationPackage,
    /// Every event cited by the hypothesis and the attestations, loaded from the
    /// store. `build` re-runs the redaction policy over each one.
    pub events: Vec<Event>,
    /// The origin's verification reports to carry as attestations.
    pub attestations: Vec<AttestationInput>,
    /// The origin lifecycle transition history, for display context.
    pub transitions: Vec<GenomeTransition>,
}

/// One verification report to carry as an attestation. `build` fingerprints the
/// report bytes; the caller supplies the report and its metadata.
pub struct AttestationInput {
    /// Evaluation family.
    pub kind: AttestationKind,
    /// Stable report identity from the origin.
    pub id: String,
    /// Stable scenario/observation set hash.
    pub set_hash: String,
    /// The complete versioned report.
    pub report: Value,
    /// Whether the origin claimed the evaluation passed.
    pub passed: bool,
    /// RFC 3339 timestamp the origin recorded the report.
    pub created_at: String,
}

/// Failure building a genome bundle.
#[derive(Debug, thiserror::Error)]
pub enum GenomeBuildError {
    /// An event was excluded by path policy. Exporting it redacted would drop it
    /// silently and break every report that cites it, so the export aborts and
    /// names the event instead.
    #[error(
        "event '{event_id}' is excluded by the current path policy; \
         it anchors evidence this genome must carry, so export cannot continue \
         (adjust import.exclude_paths or export a mutation with different evidence)"
    )]
    PathExcludedEvent {
        /// The excluded event's identity.
        event_id: String,
    },
    /// The path policy could not be compiled from the configured exclusions.
    #[error(transparent)]
    Policy(#[from] PrivacyError),
    /// The scrubbed mutation package could not be re-materialized.
    #[error("scrubbed mutation package is no longer a valid package: {0}")]
    PackageReserialize(serde_json::Error),
    /// Canonical bundle serialization failed.
    #[error("could not serialize genome bundle: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Failure parsing or integrity-checking a genome bundle.
#[derive(Debug, thiserror::Error)]
pub enum GenomeParseError {
    /// The bytes were not a valid genome bundle.
    #[error("could not parse genome bundle: {0}")]
    Json(#[from] serde_json::Error),
    /// The bundle declared an unsupported contract version.
    #[error("unsupported genome spec version '{found}', expected '{expected}'")]
    UnsupportedSpecVersion {
        /// The version the bundle declared.
        found: String,
        /// The version this binary understands.
        expected: &'static str,
    },
    /// The declared `genome_id` did not match the recomputed content id, so the
    /// bundle was altered after export.
    #[error("genome content id mismatch: declared '{declared}', computed '{computed}'")]
    GenomeIdMismatch {
        /// The id the bundle declared.
        declared: String,
        /// The id recomputed from the bundle content.
        computed: String,
    },
}

/// Build a redacted genome bundle from local materials.
///
/// Applies the redaction gate to every event (aborting on a path-excluded
/// event), scrubs the mutation's reviewable text with the same secret rules,
/// fingerprints each attestation report, and stamps the content-derived
/// `genome_id`.
///
/// # Errors
/// Returns [`GenomeBuildError`] when an event is path-excluded, the policy is
/// invalid, the scrubbed package cannot be re-materialized, or serialization
/// fails.
pub fn build(
    source: GenomeSource,
    policy: &PrivacyPolicy,
) -> Result<GenomeBundle, GenomeBuildError> {
    let mut redacted_fields = 0u64;
    let mut evidence_events: Vec<Event> = Vec::with_capacity(source.events.len());
    for event in &source.events {
        let outcome = policy.apply(event);
        match outcome.event {
            Some(sanitized) => {
                redacted_fields = redacted_fields.saturating_add(outcome.redacted_fields);
                evidence_events.push(sanitized);
            }
            None => {
                return Err(GenomeBuildError::PathExcludedEvent {
                    event_id: event.event_id.as_str().to_owned(),
                });
            }
        }
    }
    // Stable order so the content id is reproducible regardless of how the
    // caller gathered the events.
    evidence_events.sort_by(|left, right| left.event_id.as_str().cmp(right.event_id.as_str()));

    // Scrub reviewable free text (title, hypothesis, intervention instruction,
    // exclusions, failure cases) so no credential travels inside prose.
    let mut package_value = serde_json::to_value(&source.package)?;
    redacted_fields = redacted_fields.saturating_add(policy.scrub_value(&mut package_value));
    let mutation: MutationPackage =
        serde_json::from_value(package_value).map_err(GenomeBuildError::PackageReserialize)?;

    let attestations = source
        .attestations
        .into_iter()
        .map(|input| GenomeAttestation {
            kind: input.kind,
            id: input.id,
            set_hash: input.set_hash,
            content_hash: report_content_hash_hex(&input.report),
            report_json: input.report,
            passed: input.passed,
            created_at: input.created_at,
        })
        .collect();

    let mut bundle = GenomeBundle {
        spec_version: GENOME_SPEC_VERSION.to_owned(),
        genome_id: String::new(),
        exported_at: source.exported_at,
        origin: GenomeOrigin {
            instance_key: source.origin_instance_key,
            autophagy_version: source.autophagy_version,
        },
        mutation,
        evidence_events,
        attestations,
        transitions: source.transitions,
        redaction: GenomeRedaction {
            redacted_fields,
            policy: POLICY_DESCRIPTION.to_owned(),
        },
    };
    bundle.genome_id = compute_genome_id(&bundle)?;
    Ok(bundle)
}

/// Parse a genome bundle and verify its structural contract and content id.
///
/// On success the bundle is well-formed, declares the supported spec version,
/// and its `genome_id` matches the recomputed content id. Per-attestation
/// transit-integrity is checked separately via [`GenomeAttestation::hash_matches`],
/// which is a display concern rather than a parse failure.
///
/// # Errors
/// Returns [`GenomeParseError`] for malformed bytes, an unsupported spec
/// version, or a content-id mismatch (the bundle was altered after export).
pub fn parse(bytes: &[u8]) -> Result<GenomeBundle, GenomeParseError> {
    let bundle: GenomeBundle = serde_json::from_slice(bytes)?;
    if bundle.spec_version != GENOME_SPEC_VERSION {
        return Err(GenomeParseError::UnsupportedSpecVersion {
            found: bundle.spec_version,
            expected: GENOME_SPEC_VERSION,
        });
    }
    let computed = compute_genome_id(&bundle)?;
    if computed != bundle.genome_id {
        return Err(GenomeParseError::GenomeIdMismatch {
            declared: bundle.genome_id,
            computed,
        });
    }
    Ok(bundle)
}

/// The content-derived id of a bundle: `gen_` + SHA-256 hex of the bundle's
/// canonical JSON with the `genome_id` field removed. Object keys serialize in
/// sorted order, so the digest is independent of field ordering.
fn compute_genome_id(bundle: &GenomeBundle) -> Result<String, serde_json::Error> {
    let mut value = serde_json::to_value(bundle)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("genome_id");
    }
    let bytes = serde_json::to_vec(&value)?;
    Ok(format!("gen_{}", hex_digest(&bytes)))
}

/// SHA-256 hex of a report value's canonical serialization. Used for the
/// attestation transit-integrity fingerprint at both build and verify time.
#[must_use]
pub fn report_content_hash_hex(report: &Value) -> String {
    let bytes = serde_json::to_vec(report).unwrap_or_default();
    hex_digest(&bytes)
}

fn hex_digest(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut encoded = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write as _;
        write!(&mut encoded, "{byte:02x}").expect("writing to String cannot fail");
    }
    encoded
}
