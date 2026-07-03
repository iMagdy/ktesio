use std::path::Path;

use crate::error::DoctorUnhealthy;
use crate::git;
use crate::lockfile::Lockfile;
use crate::manifest::{Manifest, PublishEntry};
use crate::ui;

#[cfg(not(tarpaulin_include))]
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in(&project_root)
}

pub(crate) fn run_in(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let report = check_project(project_root);
    print_report(&report);

    if report.errors.is_empty() {
        Ok(())
    } else {
        Err(DoctorUnhealthy {
            message: format!("Project has {} skill health error(s)", report.errors.len()),
        }
        .into())
    }
}

#[derive(Debug, Default)]
struct DoctorReport {
    errors: Vec<String>,
    warnings: Vec<String>,
}

fn check_project(project_root: &Path) -> DoctorReport {
    let mut report = DoctorReport::default();

    if !git::is_git_available() {
        report
            .errors
            .push("git is not available on PATH; install git before installing skills".to_string());
    }

    let manifest_path = project_root.join("skills.json");
    let manifest = if manifest_path.exists() {
        match Manifest::load(&manifest_path) {
            Ok(manifest) => Some(manifest),
            Err(err) => {
                report.errors.push(format!(
                    "skills.json is invalid: {}. Fix the manifest JSON.",
                    err
                ));
                None
            }
        }
    } else {
        report
            .warnings
            .push("skills.json is missing; run 'kt init .' to create one.".to_string());
        None
    };

    let lockfile_path = project_root.join("skills.lock");
    let lockfile = match Lockfile::load(&lockfile_path) {
        Ok(lockfile) => Some(lockfile),
        Err(err) => {
            report.errors.push(format!(
                "skills.lock is invalid: {}. Regenerate or repair it.",
                err
            ));
            None
        }
    };

    if let Some(manifest) = &manifest {
        check_publish_entries(project_root, manifest, &mut report);
    }

    if let (Some(manifest), Some(lockfile)) = (&manifest, &lockfile) {
        for name in manifest.dependencies.keys() {
            let skill_dir = git::skill_dir(project_root, name);
            if !skill_dir.exists() && lockfile.contains(name) {
                report.errors.push(format!(
                    "skill '{}' is locked but missing from disk; run 'kt install'.",
                    name
                ));
            } else if !skill_dir.exists() {
                report.warnings.push(format!(
                    "skill '{}' is declared but not installed; run 'kt install'.",
                    name
                ));
            } else if !lockfile.contains(name) {
                report.warnings.push(format!(
                    "skill '{}' is installed but not locked; run 'kt install' or 'kt upgrade'.",
                    name
                ));
            }
        }

        for name in lockfile.entries().keys() {
            if !manifest.dependencies.contains_key(name) {
                report.warnings.push(format!(
                    "skill '{}' is orphaned in skills.lock; add it to dependencies or run 'kt uninstall {}'.",
                    name, name
                ));
            }
        }
    }

    let published_dirs = manifest
        .as_ref()
        .map(|manifest| published_skill_dir_names(project_root, manifest))
        .unwrap_or_default();

    let skills_root = project_root.join(".agents").join("skills");
    if skills_root.exists() {
        match std::fs::read_dir(&skills_root) {
            Ok(entries) => {
                for entry in entries.flatten() {
                    let Ok(file_type) = entry.file_type() else {
                        continue;
                    };
                    if !file_type.is_dir() {
                        continue;
                    }
                    let name = entry.file_name().to_string_lossy().to_string();
                    let known = manifest
                        .as_ref()
                        .map(|manifest| manifest.dependencies.contains_key(&name))
                        .unwrap_or(false)
                        || lockfile
                            .as_ref()
                            .map(|lockfile| lockfile.contains(&name))
                            .unwrap_or(false)
                        || published_dirs.contains(&name);
                    if !known {
                        report.warnings.push(format!(
                            "installed directory '{}' is untracked; run 'kt init .' to adopt it as a dependency or 'kt publish' if it should be published.",
                            name
                        ));
                    }
                }
            }
            Err(err) => report.errors.push(format!(
                "cannot read .agents/skills: {}; check directory permissions.",
                err
            )),
        }
    }

    report
}

fn check_publish_entries(project_root: &Path, manifest: &Manifest, report: &mut DoctorReport) {
    for entry in &manifest.publish {
        match entry {
            PublishEntry::Dependency(name) => {
                let Some(dependency) = manifest.dependencies.get(name) else {
                    report.errors.push(format!(
                        "publish entry '{}' does not match a dependency; add a local dependency or use 'kt publish add {} <path>'.",
                        name, name
                    ));
                    continue;
                };
                let Some(path) = dependency.path.as_deref() else {
                    report.errors.push(format!(
                        "publish entry '{}' points to a remote dependency; publish entries must use local paths.",
                        name
                    ));
                    continue;
                };
                if !project_root.join(path).exists() {
                    report.errors.push(format!(
                        "publish entry '{}' points to missing path '{}'; create it or run 'kt publish add {} <path>'.",
                        name, path, name
                    ));
                }
            }
            PublishEntry::Object(object) => {
                let publish_path = project_root.join(&object.path);
                if !publish_path.exists() {
                    report.errors.push(format!(
                        "published skill '{}' points to missing path '{}'; create it or run 'kt publish add {} <path>'.",
                        object.skill, object.path, object.skill
                    ));
                }
            }
        }
    }
}

fn published_skill_dir_names(
    project_root: &Path,
    manifest: &Manifest,
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    let skills_root = project_root.join(".agents").join("skills");
    for entry in &manifest.publish {
        let (name, path) = match entry {
            PublishEntry::Dependency(name) => {
                let Some(path) = manifest
                    .dependencies
                    .get(name)
                    .and_then(|dependency| dependency.path.as_deref())
                else {
                    continue;
                };
                (name.as_str(), path)
            }
            PublishEntry::Object(object) => (object.skill.as_str(), object.path.as_str()),
        };
        let published_path = project_root.join(path);
        if published_path == skills_root.join(name) {
            names.insert(name.to_string());
        }
    }
    names
}

fn print_report(report: &DoctorReport) {
    if report.errors.is_empty() && report.warnings.is_empty() {
        ui::success("Project skill state looks healthy.");
        return;
    }

    if !report.errors.is_empty() {
        ui::print_diagnostics("Errors", &report.errors, ui::DiagnosticKind::Error);
    }

    if !report.warnings.is_empty() {
        ui::print_diagnostics("Warnings", &report.warnings, ui::DiagnosticKind::Warning);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_doctor_healthy_empty_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_healthy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result.errors.is_empty());
        assert!(result.warnings.is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_run_in_returns_ok_when_healthy() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_run_healthy");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();

        let result = run_in(&dir);

        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_missing_manifest_warning() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_missing_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = check_project(&dir);

        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("skills.json is missing")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_print_report_handles_errors_and_warnings() {
        let report = DoctorReport {
            errors: vec!["broken".to_string()],
            warnings: vec!["stale".to_string()],
        };

        print_report(&report);
    }

    #[test]
    fn test_doctor_reports_missing_locked_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_missing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"docs": {"commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "repo": "url"}}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("missing from disk")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_invalid_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_bad_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skills.json"), "not json").unwrap();

        let result = check_project(&dir);

        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("skills.json is invalid")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_invalid_lockfile() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_bad_lock");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(dir.join("skills.lock"), "not json").unwrap();

        let result = check_project(&dir);

        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("skills.lock is invalid")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_missing_publish_path() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_missing_publish");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": [{"skill": "docs", "path": "skills/docs"}]}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("published skill 'docs'")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_publish_dependency_without_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_publish_missing_dependency");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut manifest = Manifest::new();
        manifest.add_publish_dependency("docs".to_string());
        let mut report = DoctorReport::default();

        check_publish_entries(&dir, &manifest, &mut report);

        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("does not match a dependency")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_publish_dependency_remote_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_publish_remote_dependency");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("docs".to_string(), "url".to_string(), None);
        manifest.add_publish_dependency("docs".to_string());
        let mut report = DoctorReport::default();

        check_publish_entries(&dir, &manifest, &mut report);

        assert!(report
            .errors
            .iter()
            .any(|error| error.contains("points to a remote dependency")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_publish_dependency_missing_local_path() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_publish_missing_local_path");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"path": "skills/docs"}}, "publish": ["docs"]}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .errors
            .iter()
            .any(|error| error.contains("points to missing path")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_declared_not_installed() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_manifest_only");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("declared but not installed")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_installed_not_locked() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_not_locked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("installed but not locked")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_orphaned_lockfile_entry() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_orphan");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("skills.lock"),
            r#"{"docs": {"commit": "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", "repo": "url"}}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("orphaned")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_reports_untracked_directory() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_untracked");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/local")).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result
            .warnings
            .iter()
            .any(|warning| warning.contains("untracked")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_treats_published_agents_skill_as_tracked_and_skips_files() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_published_agents_skill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();
        std::fs::write(dir.join(".agents/skills/file.md"), "# File").unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"path": ".agents/skills/docs"}}, "publish": ["docs"]}"#,
        )
        .unwrap();

        let result = check_project(&dir);

        assert!(result.errors.is_empty());
        assert!(!result
            .warnings
            .iter()
            .any(|warning| warning.contains("untracked")));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_published_skill_dir_names_ignores_unresolved_dependencies() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_published_names");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/object")).unwrap();
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("remote".to_string(), "url".to_string(), None);
        manifest.add_publish_dependency("remote".to_string());
        manifest.add_publish_dependency("missing".to_string());
        manifest.add_publish_object(
            "object".to_string(),
            ".agents/skills/object".to_string(),
            false,
        );

        let names = published_skill_dir_names(&dir, &manifest);

        assert!(names.contains("object"));
        assert!(!names.contains("remote"));
        assert!(!names.contains("missing"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_doctor_run_in_returns_error_when_unhealthy() {
        let dir = std::env::temp_dir().join("ktesio_test_doctor_run_error");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skills.json"), "not json").unwrap();

        let result = run_in(&dir);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
