use std::path::Path;

use crate::error::SkillNotFound;
use crate::lockfile::Lockfile;
use crate::manifest::Manifest;
use crate::skill;
use crate::ui;

#[cfg(not(tarpaulin_include))]
pub fn run(package_name: &str) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in(&project_root, package_name)
}

pub(crate) fn run_in(
    project_root: &Path,
    package_name: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = project_root.join("skills.json");

    let mut manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        return Err(SkillNotFound {
            message: format!("Error: skill '{}' not found in manifest", package_name),
        }
        .into());
    };

    if !manifest.has_skill(package_name) {
        return Err(SkillNotFound {
            message: format!("Error: skill '{}' not found in manifest", package_name),
        }
        .into());
    }

    manifest.remove_skill(package_name);
    manifest.save(&manifest_path)?;

    let lockfile_path = project_root.join("skills.lock");
    let mut lockfile = Lockfile::load(&lockfile_path)?;
    lockfile.remove(package_name);
    lockfile.save(&lockfile_path)?;

    skill::remove_skill_dir(project_root, package_name)?;

    ui::success(format!("Uninstalled {}", ui::skill_name(package_name)));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uninstall_no_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_uninstall_nomanifest");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, "test");
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_uninstall_not_found() {
        let dir = std::env::temp_dir().join("ktesio_test_uninstall_notfound");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let result = run_in(&dir, "nonexistent");
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_uninstall_success() {
        let dir = std::env::temp_dir().join("ktesio_test_uninstall_success");
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
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        let result = run_in(&dir, "test");
        assert!(result.is_ok());
        assert!(
            !dir.join("skills.json").exists()
                || !std::fs::read_to_string(dir.join("skills.json"))
                    .unwrap()
                    .contains("test")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_uninstall_without_lockfile() {
        let dir = std::env::temp_dir().join("ktesio_test_uninstall_nolock");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        // No lockfile
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        let result = run_in(&dir, "test");
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
