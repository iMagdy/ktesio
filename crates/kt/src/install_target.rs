use std::path::Path;

use crate::error::InstallInvalidFormat;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRepoTarget {
    pub repo: String,
    pub source_skill: Option<String>,
}

pub fn resolve_repo_target(
    input: &str,
    use_ssh: bool,
) -> Result<ResolvedRepoTarget, Box<dyn std::error::Error>> {
    if input.trim().is_empty() {
        return Err(InstallInvalidFormat {
            message: "Install target cannot be empty".to_string(),
        }
        .into());
    }

    if is_full_git_url(input) || looks_like_local_path(input) {
        return Ok(ResolvedRepoTarget {
            repo: input.to_string(),
            source_skill: None,
        });
    }

    let parts = input.split('/').collect::<Vec<_>>();
    match parts.as_slice() {
        [owner, repo] if is_github_component(owner) && is_github_component(repo) => {
            Ok(ResolvedRepoTarget {
                repo: github_clone_url(owner, repo, use_ssh),
                source_skill: None,
            })
        }
        [owner, repo, skill]
            if is_github_component(owner)
                && is_github_component(repo)
                && is_skill_component(skill) =>
        {
            Ok(ResolvedRepoTarget {
                repo: github_clone_url(owner, repo, use_ssh),
                source_skill: Some((*skill).to_string()),
            })
        }
        _ => Err(InstallInvalidFormat {
            message:
                "Invalid install target. Use name:repo, a git URL, a local path, owner/repo, or owner/repo/skill."
                    .to_string(),
        }
        .into()),
    }
}

pub fn github_clone_url(owner: &str, repo: &str, use_ssh: bool) -> String {
    let repo = repo.strip_suffix(".git").unwrap_or(repo);
    if use_ssh {
        format!("git@github.com:{owner}/{repo}.git")
    } else {
        format!("https://github.com/{owner}/{repo}.git")
    }
}

pub fn github_repo_from_source(source: &str, use_ssh: bool) -> Option<String> {
    let mut parts = source.split('/');
    let owner = parts.next()?;
    let repo = parts.next()?;
    if parts.next().is_some() || !is_github_component(owner) || !is_github_component(repo) {
        return None;
    }

    Some(github_clone_url(owner, repo, use_ssh))
}

pub fn install_target_from_source(source: &str, skill: &str) -> Option<String> {
    if github_repo_from_source(source, false).is_none() || !is_skill_component(skill) {
        return None;
    }

    Some(format!("{source}/{skill}"))
}

pub fn is_valid_skill_name(name: &str) -> bool {
    is_skill_component(name)
}

fn is_full_git_url(input: &str) -> bool {
    input.starts_with("http://")
        || input.starts_with("https://")
        || input.starts_with("ssh://")
        || input.starts_with("git@")
}

/// Broad local-path heuristic used to steer an install target away from the
/// GitHub `owner/repo` shorthand. Any backslash is a strong local-path signal
/// here because GitHub path components never contain one.
fn looks_like_local_path(input: &str) -> bool {
    is_local_path_target(input) || input.contains('\\')
}

/// Narrow local-path predicate: is this target *itself* a complete local path
/// (absolute, `./`-relative, or an existing path), as opposed to a `name:url`
/// spec whose URL half merely happens to be a local path? Used by the
/// `name:url` parser so a Windows absolute path (`C:\repo`) is not split on its
/// drive-letter colon, while `docs:C:\repo` stays a valid `name:url` spec.
/// On Unix a local path has no colon, so this only changes Windows behavior.
pub fn is_local_path_target(input: &str) -> bool {
    input.starts_with('/')
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
        || is_windows_absolute_path(input)
        || Path::new(input).exists()
}

/// Detects Windows-style absolute paths — drive-letter (`C:\`, `C:/`) and UNC
/// (`\\server\share`). Separator-agnostic classification so a Windows local
/// path is never mistaken for a `name:url` install spec. Portable: on Unix
/// these inputs simply don't occur, so this is a no-op there.
fn is_windows_absolute_path(input: &str) -> bool {
    // UNC path, e.g. \\server\share
    if input.starts_with("\\\\") {
        return true;
    }
    // Drive-letter path, e.g. C:\repo or C:/repo
    let mut chars = input.chars();
    match (chars.next(), chars.next(), chars.next()) {
        (Some(drive), Some(':'), Some(sep)) => {
            drive.is_ascii_alphabetic() && (sep == '\\' || sep == '/')
        }
        _ => false,
    }
}

/// Normalizes a relative path to use forward slashes for storage in the
/// manifest/lockfile. Persisted relative paths MUST be `/`-separated so
/// `skills.json`/`skills.lock` are portable and deterministic across OSes.
/// Only apply to relative repo-internal paths — never to repo URLs or on-disk
/// absolute `PathBuf`s. `\` is not a legal POSIX filename character, so the
/// replacement is lossless for this use.
pub fn normalize_separators_to_slash(path: &str) -> String {
    path.replace('\\', "/")
}

fn is_github_component(component: &str) -> bool {
    !component.is_empty()
        && component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
}

fn is_skill_component(component: &str) -> bool {
    !component.is_empty()
        && component
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_full_urls_and_local_paths() {
        assert_eq!(
            resolve_repo_target("https://github.com/o/r.git", false)
                .unwrap()
                .repo,
            "https://github.com/o/r.git"
        );
        assert_eq!(
            resolve_repo_target("git@github.com:o/r.git", false)
                .unwrap()
                .repo,
            "git@github.com:o/r.git"
        );
        assert_eq!(
            resolve_repo_target("/tmp/local-repo", false).unwrap().repo,
            "/tmp/local-repo"
        );
        // Windows absolute local paths must resolve verbatim as a repo target,
        // never be split on the drive-letter colon into a name:url spec.
        assert_eq!(
            resolve_repo_target(r"C:\Users\me\source", false)
                .unwrap()
                .repo,
            r"C:\Users\me\source"
        );
        assert_eq!(
            resolve_repo_target(r"\\server\share\repo", false)
                .unwrap()
                .repo,
            r"\\server\share\repo"
        );
    }

    #[test]
    fn test_normalize_separators_to_slash() {
        assert_eq!(normalize_separators_to_slash(r"a\b\c"), "a/b/c");
        assert_eq!(
            normalize_separators_to_slash(r".agents\skills\local"),
            ".agents/skills/local"
        );
        // Already-forward-slash paths (the Unix case) are untouched.
        assert_eq!(
            normalize_separators_to_slash(".agents/skills/local"),
            ".agents/skills/local"
        );
    }

    #[test]
    fn test_is_local_path_target_recognizes_windows_and_unc() {
        assert!(is_local_path_target("/abs/unix"));
        assert!(is_local_path_target("./rel"));
        assert!(is_local_path_target("../rel"));
        assert!(is_local_path_target("~/home"));
        assert!(is_local_path_target(r"C:\Users\me\source"));
        assert!(is_local_path_target("C:/Users/me/source"));
        assert!(is_local_path_target(r"\\server\share"));
        // A GitHub shorthand or bare name is not a complete local path.
        assert!(!is_local_path_target("owner/repo"));
        assert!(!is_local_path_target("nameonly"));
        // A single drive letter with no separator is not treated as absolute.
        assert!(!is_local_path_target("C:onlycolon"));
        // A `name:url` spec whose URL is a Windows path is NOT itself a local
        // path — it must stay classified as a name:url spec.
        assert!(!is_local_path_target(r"docs:C:\repos\x"));
    }

    #[test]
    fn test_looks_like_local_path_treats_backslash_as_local_signal() {
        // The broad heuristic (used to reject GitHub shorthand) also fires on a
        // stray backslash, since GitHub components never contain one.
        assert!(looks_like_local_path(r"some\relative\path"));
        assert!(looks_like_local_path(r"C:\Users\me\source"));
        assert!(!looks_like_local_path("owner/repo"));
    }

    #[test]
    fn test_resolve_github_shorthand_https_and_ssh() {
        assert_eq!(
            resolve_repo_target("hashicorp/agent-skills", false)
                .unwrap()
                .repo,
            "https://github.com/hashicorp/agent-skills.git"
        );
        assert_eq!(
            resolve_repo_target("hashicorp/agent-skills", true)
                .unwrap()
                .repo,
            "git@github.com:hashicorp/agent-skills.git"
        );
    }

    #[test]
    fn test_resolve_github_skill_shorthand() {
        let resolved =
            resolve_repo_target("hashicorp/agent-skills/run-acceptance-tests", false).unwrap();

        assert_eq!(
            resolved.repo,
            "https://github.com/hashicorp/agent-skills.git"
        );
        assert_eq!(
            resolved.source_skill.as_deref(),
            Some("run-acceptance-tests")
        );
    }

    #[test]
    fn test_resolve_invalid_shorthand() {
        assert!(resolve_repo_target("", false).is_err());
        assert!(resolve_repo_target("   ", false).is_err());
        assert!(resolve_repo_target("nameonly", false).is_err());
        assert!(resolve_repo_target("owner/repo/bad/name", false).is_err());
        assert!(resolve_repo_target("owner/repo/bad.name", false).is_err());
    }

    #[test]
    fn test_search_source_helpers() {
        assert_eq!(
            github_repo_from_source("owner/repo", false).as_deref(),
            Some("https://github.com/owner/repo.git")
        );
        assert_eq!(
            install_target_from_source("owner/repo", "skill").as_deref(),
            Some("owner/repo/skill")
        );
        assert!(github_repo_from_source("owner/repo/extra", false).is_none());
        assert!(install_target_from_source("domain.com", "skill").is_none());
    }
}
