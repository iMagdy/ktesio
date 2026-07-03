use std::path::Path;

use crate::git;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::ui;
use serde::Serialize;

#[derive(Serialize)]
struct SkillStatus {
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
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    run_with_options(false)
}

#[cfg(not(tarpaulin_include))]
pub fn run_with_options(json: bool) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in_with_options(&project_root, json)
}

#[allow(dead_code)]
pub(crate) fn run_in(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    run_in_with_options(project_root, false)
}

pub(crate) fn run_in_with_options(
    project_root: &Path,
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

    let statuses = collect_statuses(project_root, &manifest, &lockfile);

    if json {
        println!("{}", serde_json::to_string_pretty(&statuses)?);
        return Ok(());
    }

    if statuses.is_empty() {
        ui::info("No skills installed. Run 'kt install' to add skills.");
        return Ok(());
    }

    let columns = [
        ui::TableColumn::new("Name", 16, 30),
        ui::TableColumn::new("Repo", 18, 64),
        ui::TableColumn::new("Commit", 8, 10),
        ui::TableColumn::new("Status", 12, 14),
    ];
    let rows = statuses
        .iter()
        .map(|skill| {
            vec![
                ui::TableCell::skill(skill.name.as_str()),
                ui::TableCell::muted(ui::compact_source(&skill.repo)),
                ui::TableCell::muted(ui::short_commit(&skill.commit)),
                ui::TableCell::status(skill.status.as_str()),
            ]
        })
        .collect::<Vec<_>>();
    ui::print_table("Skills", &columns, &rows);

    Ok(())
}

fn collect_statuses(
    project_root: &Path,
    manifest: &Manifest,
    lockfile: &Lockfile,
) -> Vec<SkillStatus> {
    let mut statuses = Vec::new();

    for (name, entry) in &manifest.dependencies {
        let lock = lockfile.entry(name);
        let commit = lock.map(|l| l.commit.as_str()).unwrap_or("—");
        let dir = git::skill_dir(project_root, name);
        let status = if dir.exists() {
            "installed"
        } else if lock.is_some() {
            "missing"
        } else {
            "not locked"
        };

        statuses.push(SkillStatus {
            name: name.clone(),
            repo: entry
                .repo
                .clone()
                .or_else(|| entry.path.clone())
                .unwrap_or_else(|| "—".to_string()),
            skill: lock.and_then(|l| l.skill.clone()),
            commit: commit.to_string(),
            path: dir.display().to_string(),
            status: status.to_string(),
        });
    }

    for (name, lock) in lockfile.entries() {
        if !manifest.dependencies.contains_key(name) {
            statuses.push(SkillStatus {
                name: name.clone(),
                repo: lock.repo.clone(),
                skill: lock.skill.clone(),
                commit: lock.commit.clone(),
                path: git::skill_dir(project_root, name).display().to_string(),
                status: "orphaned".to_string(),
            });
        }
    }

    statuses.sort_by(|a, b| a.name.cmp(&b.name));
    statuses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_list_empty() {
        let dir = std::env::temp_dir().join("ktesio_test_list_empty");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_json_output() {
        let dir = std::env::temp_dir().join("ktesio_test_list_json");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_in_with_options(&dir, true);

        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_with_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_list_manifest");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_with_lockfile() {
        let dir = std::env::temp_dir().join("ktesio_test_list_lockfile");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"test": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#,
        )
        .unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_orphaned() {
        let dir = std::env::temp_dir().join("ktesio_test_list_orphaned");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"orphan": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#,
        )
        .unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_missing_status() {
        let dir = std::env::temp_dir().join("ktesio_test_list_missing");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"test": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#,
        )
        .unwrap();
        // Don't create the skill directory
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_list_not_locked_status() {
        let dir = std::env::temp_dir().join("ktesio_test_list_notlocked");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        // No lockfile
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
