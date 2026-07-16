//! Agent Event Protocol (AEP) types, parsing, and semantic validation.
//!
//! The normative serialized contract lives at
//! `docs/specs/aep/0.1/schema.json`. This crate deliberately contains no
//! adapter, storage, or model logic.

mod error;
mod event;
mod id;

pub use error::{EventParseError, ValidationError, ValidationErrors};
pub use event::{Artifact, ArtifactKind, Event, EventKind, SpecVersion, ToolCall};
pub use id::{EventId, SessionId};
