use std::path::{Path, PathBuf};

use dialoguer::MultiSelect;

use crate::error::{ManifestInvalidName, SkillCopyFailed};
use crate::git;
use crate::manifest::{DependencyEntry, Manifest};
use crate::ui;

#[cfg(not(tarpaulin_include))]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in(&project_root)
}

#[cfg(not(tarpaulin_include))]
pub fn run_add(skill: &str, path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_add_in(&project_root, skill, path)
}

pub(crate) fn run_in(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = project_root.join("skills.json");
    let mut manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };

    let candidates = publish_candidates(project_root, &manifest)?;
    if candidates.is_empty() {
        ui::info("No local skill dependencies found to publish.");
        if !manifest_path.exists() {
            manifest.save(&manifest_path)?;
        }
        return Ok(());
    }

    let selected = prompt_publish_selection(&candidates)?;
    if selected.is_empty() {
        ui::warning("No skills selected for publishing.");
        return Ok(());
    }

    for index in &selected {
        if let Some(candidate) = candidates.get(*index) {
            add_publish_candidate(&mut manifest, project_root, candidate)?;
        }
    }

    manifest.save(&manifest_path)?;
    ui::success(format!(
        "Published {} local skill{} in {}",
        selected.len(),
        if selected.len() == 1 { "" } else { "s" },
        manifest_path.display()
    ));
    Ok(())
}

pub(crate) fn run_add_in(
    project_root: &Path,
    skill: &str,
    path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    if !is_valid_skill_name(skill) {
        return Err(ManifestInvalidName {
            message: format!(
                "Invalid published skill name '{}': must match [a-zA-Z0-9_-]+",
                skill
            ),
        }
        .into());
    }

    let relative_path = validate_publish_path(project_root, path)?;
    let manifest_path = project_root.join("skills.json");
    let mut manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };
    manifest.add_publish_object(skill.to_string(), relative_path.clone(), false);
    manifest.save(&manifest_path)?;
    ui::success(format!(
        "Published {} from {}",
        ui::skill_name(skill),
        relative_path
    ));

    Ok(())
}

#[derive(Debug)]
struct PublishCandidate {
    name: String,
    path: PathBuf,
}

fn publish_candidates(
    project_root: &Path,
    manifest: &Manifest,
) -> Result<Vec<PublishCandidate>, Box<dyn std::error::Error>> {
    let mut candidates = Vec::new();

    for (name, dependency) in &manifest.dependencies {
        if let Some(path) = local_dependency_path(dependency) {
            let absolute = project_root.join(path);
            if absolute.exists() {
                candidates.push(PublishCandidate {
                    name: name.clone(),
                    path: absolute,
                });
            }
        }
    }

    let skills_root = project_root.join(".agents").join("skills");
    if skills_root.exists() {
        for entry in std::fs::read_dir(skills_root)? {
            let entry = entry?;
            if !entry.file_type()?.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if !is_valid_skill_name(&name) {
                continue;
            }
            if manifest.dependencies.contains_key(&name) {
                continue;
            }
            if candidates.iter().any(|candidate| candidate.name == name) {
                continue;
            }
            candidates.push(PublishCandidate {
                name,
                path: entry.path(),
            });
        }
    }

    candidates.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(candidates)
}

fn local_dependency_path(dependency: &DependencyEntry) -> Option<&str> {
    dependency.path.as_deref().filter(|path| !path.is_empty())
}

fn add_publish_candidate(
    manifest: &mut Manifest,
    project_root: &Path,
    candidate: &PublishCandidate,
) -> Result<(), Box<dyn std::error::Error>> {
    if manifest
        .dependencies
        .get(&candidate.name)
        .and_then(|dependency| dependency.path.as_deref())
        .is_none()
    {
        let project_root = project_root.canonicalize()?;
        let candidate_path = candidate.path.canonicalize()?;
        if !candidate_path.starts_with(&project_root) {
            return Err(SkillCopyFailed {
                message: format!(
                    "Publish path '{}' is outside the project",
                    candidate.path.display()
                ),
            }
            .into());
        }
        let relative_path = candidate_path
            .strip_prefix(&project_root)?
            .to_string_lossy()
            .to_string();
        manifest.add_local_dependency(candidate.name.clone(), relative_path);
    }

    manifest.add_publish_dependency(candidate.name.clone());
    Ok(())
}

#[cfg(not(tarpaulin_include))]
fn prompt_publish_selection(
    candidates: &[PublishCandidate],
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    let items = candidates
        .iter()
        .map(|candidate| format!("{} ({})", candidate.name, candidate.path.display()))
        .collect::<Vec<_>>();
    Ok(MultiSelect::new()
        .with_prompt("Select local skills to publish")
        .items(&items)
        .interact()?)
}

#[cfg(tarpaulin_include)]
fn prompt_publish_selection(
    _candidates: &[PublishCandidate],
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    Err(SkillCopyFailed {
        message: "Interactive publish selection is disabled during coverage runs".to_string(),
    }
    .into())
}

fn validate_publish_path(
    project_root: &Path,
    path: &str,
) -> Result<String, Box<dyn std::error::Error>> {
    let raw_path = Path::new(path);
    let absolute_path = if raw_path.is_absolute() {
        raw_path.to_path_buf()
    } else {
        project_root.join(raw_path)
    };

    if !absolute_path.exists() {
        return Err(SkillCopyFailed {
            message: format!("Publish path '{}' does not exist", path),
        }
        .into());
    }

    let project_root = project_root.canonicalize()?;
    let absolute_path = absolute_path.canonicalize()?;
    if !absolute_path.starts_with(&project_root) {
        return Err(SkillCopyFailed {
            message: format!("Publish path '{}' is outside the project", path),
        }
        .into());
    }

    Ok(absolute_path
        .strip_prefix(&project_root)?
        .to_string_lossy()
        .to_string())
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

#[allow(dead_code)]
fn _skill_dir(project_root: &Path, name: &str) -> PathBuf {
    git::skill_dir(project_root, name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_publish_add_creates_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_add_create");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();

        let result = run_add_in(&dir, "docs", "skills/docs");

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(manifest.publish.len(), 1);
        assert_eq!(manifest.publish[0].skill_name(), "docs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_add_updates_existing_publish_and_preserves_dependencies() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_add_update");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/new")).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": [{"skill": "local", "path": "old"}]}"#,
        )
        .unwrap();

        let result = run_add_in(&dir, "local", "skills/new");

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.dependencies.contains_key("docs"));
        assert_eq!(manifest.publish.len(), 1);
        assert_eq!(manifest.publish[0].skill_name(), "local");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_add_rejects_missing_path() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_add_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = run_add_in(&dir, "docs", "missing");

        assert!(result.is_err());
        assert!(!dir.join("skills.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_add_rejects_invalid_skill_name() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_add_invalid_name");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();

        let result = run_add_in(&dir, "bad name", "skills/docs");

        assert!(result.is_err());
        assert!(!dir.join("skills.json").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_add_accepts_absolute_path_and_rejects_outside_project() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_add_absolute");
        let outside = std::env::temp_dir().join("ktesio_test_publish_add_outside");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let result = run_add_in(
            &dir,
            "docs",
            dir.join("skills/docs").to_string_lossy().as_ref(),
        );

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert_eq!(manifest.publish[0].skill_name(), "docs");

        let result = run_add_in(&dir, "outside", outside.to_string_lossy().as_ref());

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn test_publish_run_creates_manifest_when_no_candidates() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_run_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = run_in(&dir);

        assert!(result.is_ok());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.publish.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(tarpaulin_include)]
    #[test]
    fn test_publish_run_reports_prompt_disabled_during_coverage() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_run_prompt_disabled");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();

        let result = run_in(&dir);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_candidate_adopts_untracked_installed_skill_as_local_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_candidate_adopt");
        let _ = std::fs::remove_dir_all(&dir);
        let skill_dir = dir.join(".agents/skills/local");
        std::fs::create_dir_all(&skill_dir).unwrap();

        let mut manifest = Manifest::new();
        let candidate = PublishCandidate {
            name: "local".to_string(),
            path: skill_dir,
        };

        let result = add_publish_candidate(&mut manifest, &dir, &candidate);

        assert!(result.is_ok());
        assert_eq!(
            manifest.dependencies["local"].path.as_deref(),
            Some(".agents/skills/local")
        );
        assert_eq!(manifest.publish.len(), 1);
        assert_eq!(manifest.publish[0].skill_name(), "local");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_candidates_skip_remote_dependency_even_if_installed_dir_exists() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_candidate_skip_remote");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();

        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("docs".to_string(), "url".to_string(), None);

        let candidates = publish_candidates(&dir, &manifest).unwrap();

        assert!(candidates.iter().any(|candidate| candidate.name == "local"));
        assert!(!candidates.iter().any(|candidate| candidate.name == "docs"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_candidates_filter_entries_and_include_local_dependencies() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_candidate_filters");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local/alpha")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/beta")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/bad name")).unwrap();
        std::fs::write(dir.join(".agents/skills/file.md"), "# File").unwrap();

        let mut manifest = Manifest::new();
        manifest.add_local_dependency("alpha".to_string(), "local/alpha".to_string());
        manifest.add_local_dependency("beta".to_string(), "local/missing".to_string());

        let candidates = publish_candidates(&dir, &manifest).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "alpha");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_candidates_skip_installed_skill_already_candidate() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_candidate_duplicate");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local/docs")).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();

        let mut manifest = Manifest::new();
        manifest.add_local_dependency("docs".to_string(), "local/docs".to_string());

        let candidates = publish_candidates(&dir, &manifest).unwrap();

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].name, "docs");
        assert_eq!(candidates[0].path, dir.join("local/docs"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_publish_candidate_rejects_path_outside_project() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_candidate_outside_project");
        let outside = std::env::temp_dir().join("ktesio_test_publish_candidate_outside_skill");
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&outside);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(&outside).unwrap();

        let mut manifest = Manifest::new();
        let candidate = PublishCandidate {
            name: "outside".to_string(),
            path: outside.clone(),
        };

        let result = add_publish_candidate(&mut manifest, &dir, &candidate);

        assert!(result.is_err());
        assert!(!manifest.has_dependency("outside"));
        std::fs::remove_dir_all(&dir).unwrap();
        std::fs::remove_dir_all(&outside).unwrap();
    }

    #[test]
    fn test_skill_dir_helper_uses_agents_skills_root() {
        let dir = std::env::temp_dir().join("ktesio_test_publish_skill_dir_helper");

        assert_eq!(_skill_dir(&dir, "docs"), dir.join(".agents/skills/docs"));
    }
}
