//! Engine domain core (spine AD-1).
//!
//! Pure domain: no dependency on OS-conditional code or terminal/UX crates. It
//! names the [`ProcessBackend`](crate::ports::ProcessBackend) trait (and the
//! cfg-selected backend aliases in `backends/`) but no OS type. This crate now
//! realizes the registration slice PLUS the story-1.4 lifecycle core — the
//! [`LifecycleState`] set, the data-driven transition table
//! ([`next_state`](transition::next_state)), the [`TransitionEvent`] (AD-14
//! seed), and the [`Supervisor`] that drives start/stop through the process
//! backend. Budgets and config resolution arrive with their stories.

mod error;
mod event;
mod instance;
mod lifecycle;
mod name;
mod registry;
mod supervisor;
mod transition;

pub use error::{EngineError, RegistryError};
pub use event::{TransitionCause, TransitionEvent, EVENT_SCHEMA_VERSION};
pub use instance::AgentInstance;
pub use lifecycle::LifecycleState;
pub use name::{InstanceName, NameError};
pub use registry::{Registry, RemoveDisposition};
pub use supervisor::{Supervisor, DEFAULT_STOP_WINDOW};
pub use transition::{next_state, LifecycleCommand, LifecycleError};
