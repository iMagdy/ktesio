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

fn looks_like_local_path(input: &str) -> bool {
    input.starts_with('/')
        || input.starts_with("./")
        || input.starts_with("../")
        || input.starts_with("~/")
        || Path::new(input).exists()
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
