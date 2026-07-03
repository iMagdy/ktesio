use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{LockfileInvalid, LockfileNotFound};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LockEntry {
    pub commit: String,
    pub repo: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skill: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct Lockfile {
    #[serde(flatten)]
    pub entries: HashMap<String, LockEntry>,
}

impl Lockfile {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, Box<dyn std::error::Error>> {
        if !path.exists() {
            return Ok(Self::new());
        }

        let content = fs::read_to_string(path).map_err(|_| LockfileNotFound {
            message: format!("Cannot read skills.lock at {}", path.display()),
        })?;

        Self::parse(&content)
    }

    pub fn parse(content: &str) -> Result<Self, Box<dyn std::error::Error>> {
        if content.trim().is_empty() {
            return Ok(Self::new());
        }

        let entries: HashMap<String, LockEntry> =
            serde_json::from_str(content).map_err(|e| LockfileInvalid {
                message: format!("Invalid skills.lock: {}", e),
            })?;

        for (name, entry) in &entries {
            if entry.commit.len() != 40 || !entry.commit.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(LockfileInvalid {
                    message: format!(
                        "Invalid commit hash for skill '{}': must be 40 hex characters",
                        name
                    ),
                }
                .into());
            }
        }

        Ok(Self { entries })
    }

    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        let content = serde_json::to_string_pretty(&self.entries)?;
        fs::write(path, content)
    }

    pub fn entry(&self, name: &str) -> Option<&LockEntry> {
        self.entries.get(name)
    }

    pub fn insert(&mut self, name: String, entry: LockEntry) {
        self.entries.insert(name, entry);
    }

    pub fn remove(&mut self, name: &str) -> bool {
        self.entries.remove(name).is_some()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.entries.contains_key(name)
    }

    pub fn entries(&self) -> &HashMap<String, LockEntry> {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_new() {
        let lockfile = Lockfile::new();
        assert!(lockfile.entries.is_empty());
    }

    #[test]
    fn test_default() {
        let lockfile = Lockfile::default();
        assert!(lockfile.entries.is_empty());
    }

    #[test]
    fn test_parse_valid() {
        let content = r#"{"my-skill": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url"}}"#;
        let lockfile = Lockfile::parse(content).unwrap();
        assert!(lockfile.contains("my-skill"));
    }

    #[test]
    fn test_parse_optional_source_skill() {
        let content = r#"{"my-skill": {"commit": "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", "repo": "url", "skill": "upstream-skill"}}"#;
        let lockfile = Lockfile::parse(content).unwrap();
        assert_eq!(
            lockfile.entry("my-skill").unwrap().skill.as_deref(),
            Some("upstream-skill")
        );
    }

    #[test]
    fn test_parse_empty() {
        let lockfile = Lockfile::parse("{}").unwrap();
        assert!(lockfile.entries.is_empty());
    }

    #[test]
    fn test_parse_whitespace() {
        let lockfile = Lockfile::parse("   ").unwrap();
        assert!(lockfile.entries.is_empty());
    }

    #[test]
    fn test_parse_invalid_json() {
        assert!(Lockfile::parse("not json").is_err());
    }

    #[test]
    fn test_parse_bad_commit_length() {
        let content = r#"{"s": {"commit": "abc", "repo": "url"}}"#;
        assert!(Lockfile::parse(content).is_err());
    }

    #[test]
    fn test_parse_bad_commit_hex() {
        let content =
            r#"{"s": {"commit": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz", "repo": "url"}}"#;
        assert!(Lockfile::parse(content).is_err());
    }

    #[test]
    fn test_entry() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        assert!(lockfile.entry("s").is_some());
    }

    #[test]
    fn test_entry_not_found() {
        assert!(Lockfile::new().entry("x").is_none());
    }

    #[test]
    fn test_insert() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        assert!(lockfile.contains("s"));
    }

    #[test]
    fn test_remove() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        assert!(lockfile.remove("s"));
        assert!(!lockfile.contains("s"));
    }

    #[test]
    fn test_remove_nonexistent() {
        assert!(!Lockfile::new().remove("x"));
    }

    #[test]
    fn test_contains() {
        let mut lockfile = Lockfile::new();
        assert!(!lockfile.contains("s"));
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        assert!(lockfile.contains("s"));
    }

    #[test]
    fn test_entries() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        assert_eq!(lockfile.entries().len(), 1);
    }

    #[test]
    fn test_save_and_load() {
        let mut lockfile = Lockfile::new();
        lockfile.insert(
            "s".to_string(),
            LockEntry {
                commit: "a".repeat(40),
                repo: "url".to_string(),
                skill: None,
            },
        );
        let dir = std::env::temp_dir().join("ktesio_test_lockfile");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skills.lock");
        lockfile.save(&path).unwrap();
        let loaded = Lockfile::load(&path).unwrap();
        assert!(loaded.contains("s"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_not_found() {
        let lockfile = Lockfile::load(Path::new("/nonexistent")).unwrap();
        assert!(lockfile.entries.is_empty());
    }
}
