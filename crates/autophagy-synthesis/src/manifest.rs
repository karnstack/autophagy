//! Versioned local model manifest: how a provider declares what it is.
//!
//! The manifest is a small, local JSON file. It never causes a download or a
//! network call; it only describes a model a provider may already have on disk
//! (or an endpoint the user has configured) and the capabilities it declares.
//! Loading is strict: a missing or malformed manifest produces a precise,
//! actionable error rather than a silent default.

use std::path::Path;

use serde::{Deserialize, Serialize};

/// Manifest wire-format version.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ManifestSpecVersion {
    /// Initial local model manifest contract.
    #[serde(rename = "synthesis-manifest/0.1")]
    V0_1,
}

impl ManifestSpecVersion {
    /// Stable wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::V0_1 => "synthesis-manifest/0.1",
        }
    }
}

/// Declared model packaging format.
///
/// The format is descriptive metadata only; this crate never loads a model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelFormat {
    /// A GGUF weight file, e.g. for a llama.cpp runtime.
    Gguf,
    /// An Ollama-managed local model.
    Ollama,
    /// An MLX model bundle.
    Mlx,
    /// A local OpenAI-compatible server endpoint.
    OpenAiCompatible,
    /// A deterministic, model-free reference provider.
    Deterministic,
}

impl ModelFormat {
    /// Stable wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Gguf => "gguf",
            Self::Ollama => "ollama",
            Self::Mlx => "mlx",
            Self::OpenAiCompatible => "openai_compatible",
            Self::Deterministic => "deterministic",
        }
    }
}

/// A capability a model declares it can perform.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// May propose enriched mutation candidate fields.
    MutationSynthesis,
    /// May produce local retrieval embeddings.
    Embedding,
    /// May extract structured facts from session context.
    Extraction,
}

impl Capability {
    /// Stable wire representation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MutationSynthesis => "mutation_synthesis",
            Self::Embedding => "embedding",
            Self::Extraction => "extraction",
        }
    }
}

/// Advisory resource requirements for running the model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResourceHints {
    /// Minimum resident memory, in mebibytes, required to load the model.
    pub min_memory_mb: u32,
    /// Recommended resident memory, in mebibytes, for comfortable use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recommended_memory_mb: Option<u32>,
    /// Usable context window, in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window_tokens: Option<u32>,
}

/// A versioned, local model manifest.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelManifest {
    /// Manifest contract version.
    pub spec_version: ManifestSpecVersion,
    /// Human-readable model name.
    pub name: String,
    /// Declared packaging format.
    pub format: ModelFormat,
    /// Local filesystem path or configured endpoint identifier.
    pub path: String,
    /// Model revision, tag, or version string.
    pub revision: String,
    /// Optional content digest (for example a sha256 of the weights).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
    /// Declared capabilities. At least one is required.
    pub capabilities: Vec<Capability>,
    /// Advisory resource hints.
    pub resource_hints: ResourceHints,
}

impl ModelManifest {
    /// Load and validate a manifest from a local JSON file.
    ///
    /// # Errors
    /// Returns [`ManifestError::Unreadable`] when the file cannot be read,
    /// [`ManifestError::Malformed`] when it is not valid manifest JSON, and
    /// [`ManifestError::Invalid`] when it parses but violates a semantic rule.
    pub fn from_path(path: &Path) -> Result<Self, ManifestError> {
        let bytes = std::fs::read(path).map_err(|source| ManifestError::Unreadable {
            path: path.display().to_string(),
            source,
        })?;
        let manifest: Self =
            serde_json::from_slice(&bytes).map_err(|source| ManifestError::Malformed {
                path: path.display().to_string(),
                source,
            })?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Whether the manifest declares a given capability.
    #[must_use]
    pub fn declares(&self, capability: Capability) -> bool {
        self.capabilities.contains(&capability)
    }

    /// Enforce manifest semantic invariants beyond JSON well-formedness.
    ///
    /// # Errors
    /// Returns [`ManifestError::Invalid`] listing every violation.
    pub fn validate(&self) -> Result<(), ManifestError> {
        let mut reasons = Vec::new();
        if self.name.trim().is_empty() {
            reasons.push("`name` must not be blank".to_owned());
        }
        if self.path.trim().is_empty() {
            reasons.push("`path` must not be blank".to_owned());
        }
        if self.revision.trim().is_empty() {
            reasons.push("`revision` must not be blank".to_owned());
        }
        if self
            .digest
            .as_deref()
            .is_some_and(|digest| digest.trim().is_empty())
        {
            reasons.push("`digest`, when present, must not be blank".to_owned());
        }
        if self.capabilities.is_empty() {
            reasons.push("`capabilities` must declare at least one capability".to_owned());
        }
        if self.resource_hints.min_memory_mb == 0 {
            reasons.push("`resource_hints.min_memory_mb` must be greater than zero".to_owned());
        }
        if reasons.is_empty() {
            Ok(())
        } else {
            Err(ManifestError::Invalid(reasons.join("; ")))
        }
    }
}

/// A precise, actionable manifest loading or validation failure.
#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    /// The manifest file could not be read.
    #[error("could not read model manifest at {path}: {source}")]
    Unreadable {
        /// Attempted manifest path.
        path: String,
        /// Underlying I/O error.
        #[source]
        source: std::io::Error,
    },
    /// The manifest file is not valid manifest JSON.
    #[error("model manifest at {path} is not valid synthesis-manifest/0.1 JSON: {source}")]
    Malformed {
        /// Manifest path.
        path: String,
        /// Underlying deserialization error.
        #[source]
        source: serde_json::Error,
    },
    /// The manifest parsed but violates a semantic rule.
    #[error("model manifest is invalid: {0}")]
    Invalid(String),
}
