//! Mutation Package v0.1 types, validation, and conservative candidate generation.

mod generate;
mod model;
mod validate;

pub use generate::{GenerationOutcome, equivalence_key, generate_candidate, generate_candidates};
pub use model::{
    CandidateHypothesis, GeneratedBy, Intervention, InterventionKind, LifecycleState,
    MutationPackage, MutationSpecVersion, PermissionManifest, PromotionPolicy, Trigger,
    TriggerKind,
};
pub use validate::{MutationValidationError, MutationValidationErrors};
