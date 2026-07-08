use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use indicatif::{MultiProgress, ProgressBar};

use crate::error::InitPathNotFound;
use crate::git;
use crate::lockfile::{LockEntry, Lockfile};
use crate::manifest::Manifest;
use crate::skills_sh::{self, SkillSearchResult};
use crate::ui;

const LOCAL_COMMIT: &str = "0000000000000000000000000000000000000000";

#[derive(Debug, Clone)]
struct RemoteAdoption {
    repo: String,
    commit: String,
    source_skill: Option<String>,
}

pub fn run(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    run_with_remote_resolver(path, resolve_known_skill)
}

fn run_with_remote_resolver<F>(
    path: &str,
    mut resolve_remote: F,
) -> Result<(), Box<dyn std::error::Error>>
where
    F: FnMut(&str, &ProgressBar) -> Result<Option<RemoteAdoption>, Box<dyn std::error::Error>>,
{
    let dir = Path::new(path);
    if !dir.exists() {
        return Err(InitPathNotFound {
            message: format!("Error: path '{}' does not exist", path),
        }
        .into());
    }

    let manifest_path = dir.join("skills.json");
    if manifest_path.exists() {
        ui::warning(format!(
            "skills.json already exists at {}, skipping",
            manifest_path.display()
        ));
        return Ok(());
    }

    let mut manifest = Manifest::new();
    let lockfile_path = dir.join("skills.lock");
    let mut lockfile = Lockfile::load(&lockfile_path)?;
    let skills_root = dir.join(".agents").join("skills");
    ui::info(format!(
        "Checking {} for existing skills...",
        skills_root.display()
    ));
    let lockfile_changed =
        adopt_existing_local_skills(dir, &mut manifest, &mut lockfile, &mut resolve_remote)?;
    manifest.save(&manifest_path)?;
    if lockfile_changed {
        lockfile.save(&lockfile_path)?;
    }

    ui::success(format!(
        "Created skills.json at {}",
        manifest_path.display()
    ));
    Ok(())
}

fn adopt_existing_local_skills(
    dir: &Path,
    manifest: &mut Manifest,
    lockfile: &mut Lockfile,
    resolve_remote: &mut impl FnMut(
        &str,
        &ProgressBar,
    ) -> Result<Option<RemoteAdoption>, Box<dyn std::error::Error>>,
) -> Result<bool, Box<dyn std::error::Error>> {
    let skills_root = dir.join(".agents").join("skills");
    if !skills_root.exists() {
        return Ok(false);
    }

    let mut local_adopted = 0usize;
    let mut remote_adopted = 0usize;
    let mut lockfile_changed = false;
    let mut remote_resolution_enabled = true;
    let progress = MultiProgress::new();

    for entry in std::fs::read_dir(skills_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if !is_valid_skill_name(&name) || manifest.has_dependency(&name) {
            continue;
        }

        let pb = ui::init_progress(&progress, &name);

        if let Some(entry) = lockfile.entry(&name) {
            if is_remote_lock_entry(entry) {
                manifest.add_remote_dependency(name.clone(), entry.repo.clone(), None);
                remote_adopted += 1;
                ui::finish_success(
                    &pb,
                    format!("Adopted {} from skills.lock", ui::skill_name(&name)),
                );
                continue;
            }
        }

        if remote_resolution_enabled {
            pb.set_position(10);
            pb.set_message(format!("Looking up {}", ui::skill_name(&name)));
            match resolve_remote(&name, &pb) {
                Ok(Some(remote)) if is_valid_commit(&remote.commit) => {
                    let source_skill = remote
                        .source_skill
                        .filter(|source_skill| source_skill != &name);
                    lockfile.insert(
                        name.clone(),
                        LockEntry {
                            commit: remote.commit,
                            repo: remote.repo.clone(),
                            skill: source_skill,
                        },
                    );
                    manifest.add_remote_dependency(name.clone(), remote.repo, None);
                    remote_adopted += 1;
                    lockfile_changed = true;
                    ui::finish_success(
                        &pb,
                        format!("Adopted {} as remote dependency", ui::skill_name(&name)),
                    );
                    continue;
                }
                Ok(_) => {
                    pb.set_position(85);
                    pb.set_message(format!(
                        "No public match for {}, using local path",
                        ui::skill_name(&name)
                    ));
                }
                Err(error) => {
                    remote_resolution_enabled = false;
                    pb.println(ui::warning_text(format!(
                        "Remote adoption unavailable after checking '{}': {}. Remaining existing skills will be adopted as local path dependencies.",
                        name, error
                    )));
                }
            }
        } else {
            pb.set_position(85);
            pb.set_message(format!(
                "Remote lookup skipped for {}, using local path",
                ui::skill_name(&name)
            ));
        }

        manifest.add_local_dependency(name.clone(), format!(".agents/skills/{name}"));
        local_adopted += 1;
        ui::finish_success(
            &pb,
            format!("Adopted {} as local path dependency", ui::skill_name(&name)),
        );
    }

    if remote_adopted > 0 {
        ui::info(format!(
            "Resolved {} existing skill director{} as remote dependencies.",
            remote_adopted,
            if remote_adopted == 1 { "y" } else { "ies" }
        ));
    }

    if local_adopted > 0 {
        ui::info(format!(
            "Adopted {} existing local skill director{} as dependencies.",
            local_adopted,
            if local_adopted == 1 { "y" } else { "ies" }
        ));
    }

    Ok(lockfile_changed)
}

fn resolve_known_skill(
    name: &str,
    progress: &ProgressBar,
) -> Result<Option<RemoteAdoption>, Box<dyn std::error::Error>> {
    progress.set_position(10);
    progress.set_message(format!("Looking up {} on skills.sh", ui::skill_name(name)));

    let mut notify = |message| progress.println(ui::warning_text(message));
    let results = skills_sh::search(name, 10, &mut notify)?;
    let Some(result) = exact_installable_match(name, &results) else {
        return Ok(None);
    };
    let Some(repo) = result.repo.clone() else {
        return Ok(None);
    };

    progress.set_position(25);
    progress.set_message(format!(
        "Resolving {} from {}",
        ui::skill_name(name),
        ui::compact_source(&repo)
    ));
    let commit = resolve_remote_head(&repo, name, progress)?;
    Ok(Some(RemoteAdoption {
        repo,
        commit,
        source_skill: Some(result.skill.clone()),
    }))
}

fn exact_installable_match<'a>(
    name: &str,
    results: &'a [SkillSearchResult],
) -> Option<&'a SkillSearchResult> {
    results
        .iter()
        .filter(|result| result.installable && result.repo.is_some())
        .find(|result| result.skill == name)
}

fn resolve_remote_head(
    repo: &str,
    name: &str,
    progress: &ProgressBar,
) -> Result<String, Box<dyn std::error::Error>> {
    let clone_dir = std::env::temp_dir().join(format!(
        "ktesio-init-adopt-{}-{}-{}",
        sanitize_temp_name(name),
        std::process::id(),
        unique_suffix()
    ));
    let _ = std::fs::remove_dir_all(&clone_dir);

    let result = (|| {
        git::clone_with_progress(repo, &clone_dir, progress)?;
        progress.set_position(95);
        progress.set_message(format!("Reading commit for {}", ui::skill_name(name)));
        git::rev_parse_head(&clone_dir)
    })();

    let _ = std::fs::remove_dir_all(&clone_dir);
    result
}

fn is_remote_lock_entry(entry: &LockEntry) -> bool {
    !entry.repo.trim().is_empty() && entry.commit != LOCAL_COMMIT && is_valid_commit(&entry.commit)
}

fn is_valid_commit(commit: &str) -> bool {
    commit.len() == 40 && commit.chars().all(|ch| ch.is_ascii_hexdigit())
}

fn sanitize_temp_name(name: &str) -> String {
    name.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect()
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default()
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_init_new() {
        let dir = std::env::temp_dir().join("ktesio_test_init_new");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run(dir.to_str().unwrap());
        assert!(result.is_ok());
        assert!(dir.join("skills.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_already_exists() {
        let dir = std::env::temp_dir().join("ktesio_test_init_exists");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skills.json"), "{}").unwrap();
        let result = run(dir.to_str().unwrap());
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_path_not_found() {
        let result = run("/nonexistent/path");
        assert!(result.is_err());
    }

    #[test]
    fn test_init_adopts_existing_skill_from_resolver_as_remote_dependency_and_lock() {
        let dir = std::env::temp_dir().join("ktesio_test_init_remote_adopt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();

        let result = run_with_remote_resolver(dir.to_str().unwrap(), |name, progress| {
            assert_eq!(name, "docs");
            assert!(progress.message().contains("Looking up"));
            assert!(progress.message().contains("docs"));
            Ok(Some(RemoteAdoption {
                repo: "https://github.com/example/docs.git".to_string(),
                commit: "a".repeat(40),
                source_skill: Some("docs".to_string()),
            }))
        });

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(
            manifest.dependencies["docs"].repo.as_deref(),
            Some("https://github.com/example/docs.git")
        );
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        assert_eq!(lockfile.entry("docs").unwrap().commit, "a".repeat(40));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_adopts_unmatched_existing_skill_as_local_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_init_local_adopt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();

        let result = run_with_remote_resolver(dir.to_str().unwrap(), |_, _| Ok(None));

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(
            manifest.dependencies["local"].path.as_deref(),
            Some(".agents/skills/local")
        );
        assert!(manifest.publish.is_empty());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_adopts_remote_lock_entry_without_resolver() {
        let dir = std::env::temp_dir().join("ktesio_test_init_lock_adopt");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"docs": {"commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "repo": "https://github.com/example/docs.git", "skill": "source-docs"}}"#,
        )
        .unwrap();

        let result = run_with_remote_resolver(dir.to_str().unwrap(), |_, _| {
            panic!("remote resolver should not run for a remote lock entry")
        });

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(
            manifest.dependencies["docs"].repo.as_deref(),
            Some("https://github.com/example/docs.git")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_falls_back_to_local_when_remote_commit_is_invalid() {
        let dir = std::env::temp_dir().join("ktesio_test_init_invalid_remote_commit");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();

        let result = run_with_remote_resolver(dir.to_str().unwrap(), |_, _| {
            Ok(Some(RemoteAdoption {
                repo: "https://github.com/example/local.git".to_string(),
                commit: "not-a-commit".to_string(),
                source_skill: None,
            }))
        });

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(
            manifest.dependencies["local"].path.as_deref(),
            Some(".agents/skills/local")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_disables_remote_resolution_after_error() {
        let dir = std::env::temp_dir().join("ktesio_test_init_resolver_error");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/alpha")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/beta")).unwrap();
        let mut calls = 0usize;

        let result = run_with_remote_resolver(dir.to_str().unwrap(), |_, _| {
            calls += 1;
            Err("skills.sh unavailable".into())
        });

        assert!(result.is_ok());
        assert_eq!(calls, 1);
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(
            manifest.dependencies["alpha"].path.as_deref(),
            Some(".agents/skills/alpha")
        );
        assert_eq!(
            manifest.dependencies["beta"].path.as_deref(),
            Some(".agents/skills/beta")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_skips_files_invalid_names_and_existing_dependencies() {
        let dir = std::env::temp_dir().join("ktesio_test_init_skips_entries");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/bad name")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();
        std::fs::write(dir.join(".agents/skills/file.md"), "# File").unwrap();
        let mut manifest = Manifest::new();
        let mut lockfile = Lockfile::default();
        manifest.add_local_dependency("local".to_string(), ".agents/skills/local".to_string());

        let changed =
            adopt_existing_local_skills(&dir, &mut manifest, &mut lockfile, &mut |_, _| Ok(None))
                .unwrap();

        assert!(!changed);
        assert_eq!(manifest.dependencies.len(), 1);
        assert!(manifest.has_dependency("local"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_init_helpers_match_installable_results_and_sanitize_names() {
        let results = vec![
            SkillSearchResult {
                id: "one".to_string(),
                name: "one".to_string(),
                source: "example/one".to_string(),
                skill: "docs".to_string(),
                repo: None,
                installs: 0,
                url: None,
                install_target: None,
                installable: true,
            },
            SkillSearchResult {
                id: "two".to_string(),
                name: "two".to_string(),
                source: "example/two".to_string(),
                skill: "docs".to_string(),
                repo: Some("https://github.com/example/docs.git".to_string()),
                installs: 0,
                url: None,
                install_target: Some("example/docs".to_string()),
                installable: true,
            },
        ];

        let matched = exact_installable_match("docs", &results).unwrap();
        assert_eq!(
            matched.repo.as_deref(),
            Some("https://github.com/example/docs.git")
        );
        assert_eq!(sanitize_temp_name("bad name!"), "bad-name-");
        assert!(unique_suffix() > 0);
        assert!(is_valid_commit(&"a".repeat(40)));
        assert!(!is_valid_commit("short"));
    }

    #[test]
    fn test_resolve_remote_head_clones_reads_commit_and_cleans_up() {
        let dir = std::env::temp_dir().join("ktesio_test_init_resolve_remote_head");
        let _ = std::fs::remove_dir_all(&dir);
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("README.md"), "fixture").unwrap();
        run_git(&repo, &["init"]);
        run_git(&repo, &["add", "."]);
        run_git(
            &repo,
            &[
                "-c",
                "user.name=ktesio tests",
                "-c",
                "user.email=ktesio-tests@example.com",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-m",
                "initial fixture",
            ],
        );
        let expected = git::rev_parse_head(&repo).unwrap();
        let progress = ProgressBar::hidden();

        let actual = resolve_remote_head(repo.to_str().unwrap(), "docs", &progress).unwrap();

        assert_eq!(actual, expected);
        assert_eq!(progress.position(), 95);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_resolve_known_skill_propagates_short_query_error_without_network() {
        let progress = ProgressBar::hidden();

        let result = resolve_known_skill("x", &progress);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("at least 2"));
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = Command::new("git")
            .args(args)
            .current_dir(repo)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
