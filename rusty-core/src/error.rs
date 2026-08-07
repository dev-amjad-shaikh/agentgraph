//! Error types for Rusty Core.
//!
//! All fallible operations in the crate return [`Result<T>`] with
//! [`RustyError`] as the error type.

use serde_json::Value;
use thiserror::Error;

/// The single error type for the whole crate.
///
/// Design note: `Interrupt` is modeled as an *error* because a node calls
/// `interrupt()` mid-execution to unwind out of the current super-step; the
/// executor catches this variant, persists a checkpoint, and surfaces the
/// payload to the caller. This mirrors LangGraph's `GraphInterrupt` and keeps
/// the control flow explicit in the type system.
#[derive(Debug, Error)]
pub enum RustyError {
    /// Structural graph problems: invalid builder usage, validation failures
    /// from `GraphBuilder::compile()` (unknown entry point, dangling edges), routing
    /// to unknown nodes at runtime, exceeded `max_steps`, etc.
    #[error("graph error: {0}")]
    Graph(String),

    /// A node failed during execution. The string should include the node
    /// name and the underlying failure description.
    #[error("node error: {0}")]
    Node(String),

    /// A node invoked `interrupt(payload)`. Carries the payload to surface
    /// to the caller; resumable via `RunConfig::resume` / `Command::resume`.
    ///
    /// This variant is **not** a failure — it is the suspend signal of the
    /// interrupt/resume protocol.
    #[error("graph interrupted")]
    Interrupt {
        /// The payload passed to `interrupt()`, surfaced to the caller
        /// (e.g. a human-in-the-loop approval request).
        value: Value,
    },

    /// Checkpoint persistence failures (IO, serialization, not found).
    #[error("checkpoint error: {0}")]
    Checkpoint(String),

    /// LLM provider failures (HTTP errors, malformed responses, auth).
    #[error("llm error: {0}")]
    Llm(String),

    /// Tool execution failures (unknown tool, bad arguments, runtime error).
    #[error("tool error: {0}")]
    Tool(String),

    /// JSON (de)serialization failures.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    /// A channel received an update it cannot accept — most commonly a
    /// `LastValue`-style channel written more than once in a single
    /// super-step (the LangGraph `InvalidUpdateError` class of bug), or a
    /// write to an undeclared channel.
    #[error("invalid state update: {0}")]
    InvalidUpdate(String),
}

impl RustyError {
    /// Returns `true` if this error is an interrupt (suspend) signal rather
    /// than an actual failure.
    pub fn is_interrupt(&self) -> bool {
        matches!(self, RustyError::Interrupt { .. })
    }

    /// If this is an [`RustyError::Interrupt`], returns a reference to
    /// the interrupt payload.
    pub fn interrupt_value(&self) -> Option<&Value> {
        match self {
            RustyError::Interrupt { value } => Some(value),
            _ => None,
        }
    }
}

/// Crate-wide result alias.
pub type Result<T> = std::result::Result<T, RustyError>;
