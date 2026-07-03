use std::path::Path;

use indicatif::MultiProgress;

use crate::git;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::ui;

#[cfg(not(tarpaulin_include))]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in(&project_root)
}

pub(crate) fn run_in(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = project_root.join("skills.json");
    let lockfile_path = project_root.join("skills.lock");

    let manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };

    let mut lockfile = Lockfile::load(&lockfile_path)?;

    let skills_to_upgrade: Vec<(String, String)> = if !lockfile.entries().is_empty() {
        lockfile
            .entries()
            .iter()
            .map(|(n, e)| (n.clone(), e.repo.clone()))
            .collect()
    } else {
        manifest
            .dependencies
            .iter()
            .filter_map(|(n, e)| e.repo.clone().map(|repo| (n.clone(), repo)))
            .collect()
    };

    if skills_to_upgrade.is_empty() {
        ui::info("No skills to upgrade.");
        return Ok(());
    }

    let mp = MultiProgress::new();
    let mut errors: Vec<String> = Vec::new();

    for (name, _repo_url) in &skills_to_upgrade {
        let skill_dir = git::skill_dir(project_root, name);

        if !skill_dir.exists() {
            errors.push(format!(
                "Error upgrading {}: directory does not exist",
                name
            ));
            ui::warning(format!(
                "Skipping {} because the skill directory is missing",
                ui::skill_name(name)
            ));
            continue;
        }

        let pb = ui::upgrade_progress(&mp, name);
        pb.set_position(15);
        pb.set_message(format!("Fetching {}", ui::skill_name(name)));

        if let Err(e) = git::fetch(&skill_dir) {
            ui::finish_error(&pb, format!("Error fetching {}: {}", name, e));
            errors.push(format!("Error fetching {}: {}", name, e));
            continue;
        }

        pb.set_position(65);
        pb.set_message(format!("Checking out {}", ui::skill_name(name)));
        if let Err(e) = git::checkout_default_branch(&skill_dir) {
            ui::finish_error(&pb, format!("Error checking out {}: {}", name, e));
            errors.push(format!("Error checking out {}: {}", name, e));
            continue;
        }

        pb.set_position(90);
        pb.set_message(format!("Recording {}", ui::skill_name(name)));
        let commit = git::rev_parse_head(&skill_dir).unwrap_or_default();
        if let Some(entry) = lockfile.entry(name) {
            let new_entry = crate::lockfile::LockEntry {
                commit,
                repo: entry.repo.clone(),
                skill: entry.skill.clone(),
            };
            lockfile.insert(name.to_string(), new_entry);
        }

        ui::finish_success(&pb, format!("Upgraded {}", ui::skill_name(name)));
    }

    lockfile.save(&lockfile_path)?;

    if !errors.is_empty() {
        eprintln!();
        ui::error("Errors encountered:");
        for err in &errors {
            eprintln!("  {}", err);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn test_upgrade_empty() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_empty");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_upgrade_with_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_manifest");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_upgrade_with_lockfile() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_lockfile");
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
        let result = run_in(&dir);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_upgrade_nonexistent_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_nonexist");
        std::fs::create_dir_all(&dir).unwrap();
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
    fn test_upgrade_fetch_fails() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_fetchfail");
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
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
    fn test_upgrade_success_updates_lockfile_commit() {
        let dir = std::env::temp_dir().join("ktesio_test_upgrade_success");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = create_local_repo(&dir, "source");
        let skill_dir = dir.join(".agents/skills/test");
        std::fs::create_dir_all(skill_dir.parent().unwrap()).unwrap();
        run_git(
            &dir,
            &[
                "clone",
                source.to_str().unwrap(),
                skill_dir.to_str().unwrap(),
            ],
        );
        let old_commit = "a".repeat(40);
        std::fs::write(
            dir.join("skills.lock"),
            format!(
                r#"{{"test": {{"commit": "{}", "repo": "{}"}}}}"#,
                old_commit,
                source.display()
            ),
        )
        .unwrap();

        let result = run_in(&dir);

        assert!(result.is_ok());
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        let entry = lockfile.entry("test").unwrap();
        assert_ne!(entry.commit, old_commit);
        assert_eq!(entry.repo, source.to_string_lossy());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn create_local_repo(root: &std::path::Path, name: &str) -> std::path::PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(repo.join("SKILL.md"), "# Test").unwrap();
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
        repo
    }

    fn run_git(repo: &std::path::Path, args: &[&str]) {
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
