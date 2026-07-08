use std::path::Path;

use crate::error::SkillNotFound;
use crate::git;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::ui;
use serde::Serialize;

#[derive(Serialize)]
struct SkillDetails {
    name: String,
    repo: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skill: Option<String>,
    commit: String,
    path: String,
    status: String,
}

#[cfg(not(tarpaulin_include))]
#[allow(dead_code)]
pub fn run(package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    run_with_options(package_name, false)
}

#[cfg(not(tarpaulin_include))]
pub fn run_with_options(package_name: &str, json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in_with_options(&project_root, package_name, json)
}

#[allow(dead_code)]
pub(crate) fn run_in(
    project_root: &Path,
    package_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    run_in_with_options(project_root, package_name, false)
}

pub(crate) fn run_in_with_options(
    project_root: &Path,
    package_name: &str,
    json: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = project_root.join("skills.json");
    let lockfile_path = project_root.join("skills.lock");

    let manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };

    let lockfile = Lockfile::load(&lockfile_path)?;

    let entry = manifest.dependencies.get(package_name);
    let lock = lockfile.entry(package_name);

    if entry.is_none() && lock.is_none() {
        return Err(SkillNotFound {
            message: format!("Error: skill '{}' not found", package_name),
        }
        .into());
    }

    let repo = entry
        .and_then(|e| e.repo.as_deref().or(e.path.as_deref()))
        .or_else(|| lock.map(|l| l.repo.as_str()))
        .unwrap_or("—");
    let source_skill = lock.and_then(|l| l.skill.clone());
    let commit = lock.map(|l| l.commit.as_str()).unwrap_or("—");
    let dir = git::skill_dir(project_root, package_name);
    let status = if dir.exists() {
        "installed"
    } else if lock.is_some() {
        "missing"
    } else {
        "not installed"
    };

    let details = SkillDetails {
        name: package_name.to_string(),
        repo: repo.to_string(),
        skill: source_skill,
        commit: commit.to_string(),
        path: dir.display().to_string(),
        status: status.to_string(),
    };

    if json {
        println!("{}", serde_json::to_string_pretty(&details)?);
    } else {
        println!(
            "{} {}",
            ui::table_header("Skill"),
            ui::skill_name(package_name)
        );
        println!("{} {}", ui::label("Repo   "), details.repo);
        if let Some(source_skill) = &details.skill {
            println!("{} {}", ui::label("Skill  "), source_skill);
        }
        println!("{} {}", ui::label("Commit "), details.commit);
        println!("{} {}", ui::label("Path   "), details.path);
        println!(
            "{} {}",
            ui::label("Status "),
            ui::status_label(&details.status)
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_not_found() {
        let dir = std::env::temp_dir().join("ktesio_test_show_notfound");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, "nonexistent");
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_found() {
        let dir = std::env::temp_dir().join("ktesio_test_show_found");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        let result = run_in(&dir, "test");
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_json_output() {
        let dir = std::env::temp_dir().join("ktesio_test_show_json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_in_with_options(&dir, "test", true);

        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_in_lockfile_only() {
        let dir = std::env::temp_dir().join("ktesio_test_show_lockonly");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"test": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#,
        )
        .unwrap();
        let result = run_in(&dir, "test");
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_prints_source_skill_from_lockfile() {
        let dir = std::env::temp_dir().join("ktesio_test_show_source_skill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"test": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url", "skill": "source"}}"#,
        )
        .unwrap();

        let result = run_in(&dir, "test");

        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_missing_status() {
        let dir = std::env::temp_dir().join("ktesio_test_show_missing");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"test": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#,
        )
        .unwrap();
        // Don't create the skill directory
        let result = run_in(&dir, "test");
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_show_manifest_only_not_installed_status() {
        let dir = std::env::temp_dir().join("ktesio_test_show_manifest_only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_in(&dir, "test");

        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
