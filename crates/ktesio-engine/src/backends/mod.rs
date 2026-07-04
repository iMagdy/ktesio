//! Per-OS process backends (spine AD-4) — THE cfg boundary.
//!
//! This module and its `unix/` / `windows/` children are the SOLE location
//! where OS-conditional compilation (`#[cfg(unix)]` / `#[cfg(windows)]` /
//! `target_os`) is allowed — the OS-cfg CI gate allowlists exactly
//! `^crates/ktesio-engine/src/backends/`. Everywhere else (the supervisor,
//! ports, domain, `kt`) names the [`ProcessBackend`](crate::ports::ProcessBackend)
//! trait and the cfg-selected [`Backend`] / [`Handle`] aliases below — never a
//! concrete backend, never an OS type, never a `#[cfg]`.
//!
//! ## The selection
//!
//! [`Backend`] resolves to the Unix backend on any Unix target and the Windows
//! backend on Windows. [`current`] constructs the one for the running target.
//! The supervisor stores running processes as `HashMap<_, `[`Handle`]`>` and
//! calls the port methods; the concrete OS resources (process groups, Job
//! Objects) are hidden inside the selected backend.

use crate::ports::ProcessBackend;

#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

// ---- The cfg-selected concrete backend + its handle (named via aliases) ----

/// The concrete [`ProcessBackend`] for the current target (Unix flavor).
#[cfg(unix)]
pub type Backend = unix::UnixBackend;

/// The concrete [`ProcessBackend`] for the current target (Windows flavor).
#[cfg(windows)]
pub type Backend = windows::WindowsBackend;

/// The running-process handle type for the current target's [`Backend`].
///
/// The supervisor stores these without ever naming an OS type — this alias IS
/// the seam.
pub type Handle = <Backend as ProcessBackend>::Handle;

/// Construct the process backend for the running operating system (AD-4).
///
/// The only per-OS selection point. Callers get a value typed as [`Backend`]
/// and drive it through the [`ProcessBackend`] trait.
pub fn current() -> Backend {
    Backend::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_backend_constructs() {
        // On any host CI runs, `current()` must build the target's backend.
        // (Behavioral spawn/stop coverage lives in the supervisor + conformance
        // integration tests; this only proves the selection compiles + builds.)
        let _backend = current();
    }
}
