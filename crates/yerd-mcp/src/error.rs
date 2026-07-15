//! Argument-validation errors for `tools/call`.
//!
//! These are surfaced to the agent as JSON-RPC `-32602` (invalid params)
//! rather than as tool results: a malformed call is a protocol-level mistake by
//! the client, not a failed operation.

/// Why a `tools/call` could not be turned into a [`yerd_ipc::Request`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArgError {
    /// The tool name is not in the catalog.
    #[error("unknown tool `{0}`")]
    UnknownTool(String),
    /// A required argument was absent.
    #[error("missing required argument `{0}`")]
    Missing(&'static str),
    /// An argument was present but of the wrong JSON type.
    #[error("argument `{name}` must be {expected}")]
    Type {
        /// The argument name.
        name: &'static str,
        /// A human description of the expected type, e.g. `"a string"`.
        expected: &'static str,
    },
    /// An argument was not one of an enumerated set of values.
    #[error("argument `{name}` must be one of: {allowed}")]
    NotAllowed {
        /// The argument name.
        name: &'static str,
        /// The permitted values, comma-separated.
        allowed: String,
    },
    /// An argument had the right type but an invalid value.
    #[error("argument `{name}` is invalid: {reason}")]
    Invalid {
        /// The argument name.
        name: &'static str,
        /// Why the value was rejected.
        reason: String,
    },
}
