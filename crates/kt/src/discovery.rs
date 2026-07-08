use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Type of discovered skill source
#[derive(Debug, Clone, PartialEq)]
pub enum SkillType {
    /// Single `.md` file
    File,
    /// Directory containing skill files
    Directory,
}

/// A skill found during fallback discovery
#[derive(Debug, Clone)]
pub struct DiscoveredSkill {
    /// Normalized display name (file/dir name, cleaned)
    pub name: String,
    /// Original path in the repository
    pub path: PathBuf,
    /// Whether source is file or directory
    pub skill_type: SkillType,
}

/// Result of the fallback discovery process
#[derive(Debug)]
pub struct DiscoveryResult {
    /// List of discovered skills (deduplicated)
    pub skills: Vec<DiscoveredSkill>,
    /// Warnings encountered during discovery
    pub warnings: Vec<String>,
}

/// Normalize a skill name from a filename or directory name.
/// Strips `.md`, converts whitespace to `-`, and keeps manifest-safe ASCII.
pub fn normalize_skill_name(name: &str) -> String {
    let stem = name.strip_suffix(".md").unwrap_or(name);
    let mut normalized = String::new();
    let mut last_was_dash = false;

    for ch in stem.chars() {
        let next = if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            Some(ch)
        } else if ch.is_whitespace() {
            Some('-')
        } else {
            None
        };

        if let Some(ch) = next {
            if ch == '-' {
                if last_was_dash {
                    continue;
                }
                last_was_dash = true;
            } else {
                last_was_dash = false;
            }
            normalized.push(ch);
        }
    }

    normalized.trim_matches('-').to_string()
}

/// Find the skills directory in a repository root.
/// Root-level skills directories take precedence over `.agents/skills`.
/// Returns the path to the skills directory if found.
pub fn find_skills_directory(repo_root: &Path) -> Option<PathBuf> {
    for preferred in ["skills", "SKILLS"] {
        let path = repo_root.join(preferred);
        if path.is_dir() {
            return Some(path);
        }
    }

    let mut root_matches = Vec::new();
    let entries = std::fs::read_dir(repo_root).ok()?;

    for entry in entries.flatten() {
        let file_type = entry.file_type().ok()?;
        if !file_type.is_dir() {
            continue;
        }

        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if name_str.to_lowercase() == "skills" && name_str != "skills" && name_str != "SKILLS" {
            root_matches.push(entry.path());
        }
    }

    root_matches.sort();
    if let Some(path) = root_matches.into_iter().next() {
        return Some(path);
    }

    let agents_skills_dir = repo_root.join(".agents").join("skills");
    if agents_skills_dir.is_dir() {
        return Some(agents_skills_dir);
    }

    None
}

/// Discover skills in a directory by scanning for .md files and subdirectories.
pub fn discover_skills(skills_dir: &Path) -> DiscoveryResult {
    let mut skills = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_names = HashSet::new();

    let entries = match std::fs::read_dir(skills_dir) {
        Ok(entries) => entries,
        Err(e) => {
            warnings.push(format!("Failed to read skills directory: {}", e));
            return DiscoveryResult { skills, warnings };
        }
    };

    for entry in entries.flatten() {
        let file_type = match entry.file_type() {
            Ok(ft) => ft,
            Err(_) => continue,
        };

        let file_name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden files and common non-skill directories
        if file_name.starts_with('.') || file_name == "target" || file_name == "node_modules" {
            continue;
        }

        let skill_type = if file_type.is_dir() {
            SkillType::Directory
        } else if file_name.ends_with(".md") {
            SkillType::File
        } else {
            continue; // Skip non-.md files and other non-directory entries
        };

        let normalized_name = normalize_skill_name(&file_name);

        if normalized_name.is_empty() {
            warnings.push(format!(
                "Skill '{}' skipped because its name is invalid",
                file_name
            ));
            continue;
        }

        // Deduplicate by normalized name
        if !seen_names.insert(normalized_name.clone()) {
            warnings.push(format!("Duplicate skill '{}' skipped", normalized_name));
            continue;
        }

        skills.push(DiscoveredSkill {
            name: normalized_name,
            path: entry.path(),
            skill_type,
        });
    }

    // Sort by name for consistent display
    skills.sort_by(|a, b| a.name.cmp(&b.name));

    DiscoveryResult { skills, warnings }
}

/// Deduplicate a list of discovered skills by normalized name.
/// Keeps the first occurrence and collects warnings for duplicates.
#[cfg(test)]
pub fn deduplicate_skills(skills: Vec<DiscoveredSkill>) -> DiscoveryResult {
    let mut unique = Vec::new();
    let mut warnings = Vec::new();
    let mut seen_names = HashSet::new();

    for skill in skills {
        if seen_names.insert(skill.name.clone()) {
            unique.push(skill);
        } else {
            warnings.push(format!("Duplicate skill '{}' skipped", skill.name));
        }
    }

    DiscoveryResult {
        skills: unique,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_skill_name_with_extension() {
        assert_eq!(normalize_skill_name("web-perf.md"), "web-perf");
    }

    #[test]
    fn test_normalize_skill_name_without_extension() {
        assert_eq!(normalize_skill_name("ui-ux-pro-max"), "ui-ux-pro-max");
    }

    #[test]
    fn test_normalize_skill_name_underscores() {
        assert_eq!(normalize_skill_name("agents_sdk.md"), "agents_sdk");
    }

    #[test]
    fn test_normalize_skill_name_mixed() {
        assert_eq!(normalize_skill_name("my_cool-skill.md"), "my_cool-skill");
    }

    #[test]
    fn test_normalize_skill_name_strips_invalid_characters() {
        assert_eq!(normalize_skill_name(" @@@ "), "");
        assert_eq!(normalize_skill_name("docs/api!.md"), "docsapi");
        assert_eq!(normalize_skill_name("My  Skill.md"), "My-Skill");
    }

    #[test]
    fn test_normalize_skill_name_no_changes() {
        assert_eq!(normalize_skill_name("simple"), "simple");
    }

    #[test]
    fn test_find_skills_directory_found() {
        let dir = std::env::temp_dir().join("ktesio_test_find_skills");
        let skills_dir = dir.join("skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let result = find_skills_directory(&dir);
        assert!(result.is_some());
        assert_eq!(result.unwrap(), skills_dir);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_skills_directory_case_insensitive() {
        let dir = std::env::temp_dir().join("ktesio_test_find_skills_case");
        let skills_dir = dir.join("SKILLS");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let result = find_skills_directory(&dir);
        assert!(result.is_some());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_skills_directory_finds_agents_skills() {
        let dir = std::env::temp_dir().join("ktesio_test_find_agents_skills");
        let _ = std::fs::remove_dir_all(&dir);
        let skills_dir = dir.join(".agents/skills");
        std::fs::create_dir_all(&skills_dir).unwrap();

        let result = find_skills_directory(&dir);
        assert_eq!(result, Some(skills_dir));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_skills_directory_prefers_root_skills() {
        let dir = std::env::temp_dir().join("ktesio_test_find_prefers_root_skills");
        let _ = std::fs::remove_dir_all(&dir);
        let root_skills_dir = dir.join("skills");
        std::fs::create_dir_all(&root_skills_dir).unwrap();
        std::fs::create_dir_all(dir.join(".agents/skills")).unwrap();

        let result = find_skills_directory(&dir);
        assert_eq!(result, Some(root_skills_dir));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_skills_directory_ignores_agents_without_skills() {
        let dir = std::env::temp_dir().join("ktesio_test_find_agents_without_skills");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents")).unwrap();

        let result = find_skills_directory(&dir);
        assert!(result.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_find_skills_directory_not_found() {
        let dir = std::env::temp_dir().join("ktesio_test_find_skills_none");
        std::fs::create_dir_all(&dir).unwrap();

        let result = find_skills_directory(&dir);
        assert!(result.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_md_files() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_md");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("web-perf.md"), "content").unwrap();
        std::fs::write(dir.join("ui-ux.md"), "content").unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 2);
        assert!(result.warnings.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_directories() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_dirs");
        std::fs::create_dir_all(dir.join("skill-a")).unwrap();
        std::fs::create_dir_all(dir.join("skill-b")).unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_mixed() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_mixed");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("file-skill.md"), "content").unwrap();
        std::fs::create_dir_all(dir.join("dir-skill")).unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 2);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_deduplication() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_dedup");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skill.md"), "content1").unwrap();
        std::fs::create_dir_all(dir.join("skill")).unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 1);
        assert!(!result.warnings.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_skips_hidden() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_hidden");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(".hidden.md"), "content").unwrap();
        std::fs::write(dir.join("visible.md"), "content").unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 1);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_empty_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_empty");
        std::fs::create_dir_all(&dir).unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 0);
        assert!(result.warnings.is_empty());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_non_md_files() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_nonmd");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("script.js"), "content").unwrap();
        std::fs::write(dir.join("style.css"), "content").unwrap();
        std::fs::write(dir.join("readme.txt"), "content").unwrap();

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 0);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_skills_unreadable_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_discover_unreadable");
        // Don't create the directory

        let result = discover_skills(&dir);
        assert_eq!(result.skills.len(), 0);
        assert!(!result.warnings.is_empty());

        // No cleanup needed since directory doesn't exist
    }

    #[test]
    fn test_deduplicate_skills_no_duplicates() {
        let skills = vec![
            DiscoveredSkill {
                name: "skill1".to_string(),
                path: PathBuf::from("/a"),
                skill_type: SkillType::File,
            },
            DiscoveredSkill {
                name: "skill2".to_string(),
                path: PathBuf::from("/b"),
                skill_type: SkillType::Directory,
            },
        ];

        let result = deduplicate_skills(skills);
        assert_eq!(result.skills.len(), 2);
        assert!(result.warnings.is_empty());
    }

    #[test]
    fn test_deduplicate_skills_reports_duplicates() {
        let skills = vec![
            DiscoveredSkill {
                name: "skill".to_string(),
                path: PathBuf::from("/a"),
                skill_type: SkillType::File,
            },
            DiscoveredSkill {
                name: "skill".to_string(),
                path: PathBuf::from("/b"),
                skill_type: SkillType::Directory,
            },
        ];

        let result = deduplicate_skills(skills);

        assert_eq!(result.skills.len(), 1);
        assert_eq!(result.warnings, vec!["Duplicate skill 'skill' skipped"]);
    }

    #[test]
    fn test_deduplicate_skills_empty() {
        let skills = vec![];
        let result = deduplicate_skills(skills);
        assert_eq!(result.skills.len(), 0);
        assert!(result.warnings.is_empty());
    }
}
