use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::self_update::failed))]
pub struct SelfUpdateFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::duplicate_name))]
pub struct AgentDuplicateName {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::invalid_name))]
pub struct AgentInvalidName {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::not_found))]
pub struct AgentNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::running_requires_force))]
pub struct AgentRunningRequiresForce {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::io))]
pub struct AgentIo {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::store))]
pub struct AgentStore {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::unknown_kind))]
pub struct AgentUnknownKind {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::manifest_not_found))]
pub struct AgentManifestNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::manifest_invalid))]
pub struct AgentManifestInvalid {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::manifest_unreadable))]
pub struct AgentManifestUnreadable {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::no_metering_source))]
pub struct AgentNoMeteringSource {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::no_capabilities))]
pub struct AgentNoCapabilities {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::invalid_transition))]
pub struct AgentInvalidTransition {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::launch_failed))]
pub struct AgentLaunchFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::capability_unsupported))]
pub struct AgentCapabilityUnsupported {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::unknown_config_key))]
pub struct AgentUnknownConfigKey {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::agent::config))]
pub struct AgentConfig {
    pub message: String,
}
