//! Engine domain core (spine AD-1).
//!
//! Pure domain: no dependency on adapters, OS-conditional code, or terminal/UX
//! crates. This story realizes the registration slice — Lifecycle State,
//! the [`AgentInstance`] entity, the [`InstanceName`] newtype, the
//! [`RegistryError`] type, and the [`Registry`] service. The full lifecycle
//! transition table, budgets, and config resolution arrive with their stories
//! (entity-timing).

mod error;
mod instance;
mod lifecycle;
mod name;
mod registry;

pub use error::RegistryError;
pub use instance::AgentInstance;
pub use lifecycle::LifecycleState;
pub use name::{InstanceName, NameError};
pub use registry::{Registry, RemoveDisposition};
