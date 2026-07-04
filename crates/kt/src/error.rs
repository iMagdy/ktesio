use miette::Diagnostic;
use thiserror::Error;

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::init::path_not_found))]
pub struct InitPathNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::manifest::not_found))]
pub struct ManifestNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::manifest::duplicate_name))]
pub struct ManifestDuplicateName {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::manifest::invalid_name))]
pub struct ManifestInvalidName {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::lockfile::not_found))]
pub struct LockfileNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::lockfile::invalid))]
pub struct LockfileInvalid {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::git::clone_failed))]
pub struct GitCloneFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::git::fetch_failed))]
pub struct GitFetchFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::git::checkout_failed))]
pub struct GitCheckoutFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::git::rev_parse_failed))]
pub struct GitRevParseFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::skill::copy_failed))]
pub struct SkillCopyFailed {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::skill::not_found))]
pub struct SkillNotFound {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::install::invalid_format))]
pub struct InstallInvalidFormat {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::install::already_exists))]
pub struct InstallAlreadyExists {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::discovery::error))]
pub struct DiscoveryError {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::discovery::skills_directory_empty))]
pub struct SkillsDirectoryEmpty {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::doctor::unhealthy))]
pub struct DoctorUnhealthy {
    pub message: String,
}

#[derive(Error, Diagnostic, Debug)]
#[error("{}", message)]
#[diagnostic(code(ktesio::search::failed))]
pub struct SearchFailed {
    pub message: String,
}

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
