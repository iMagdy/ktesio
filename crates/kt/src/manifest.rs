use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{ManifestDuplicateName, ManifestInvalidName, ManifestNotFound};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DependencyEntry {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rev: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PublishObject {
    pub skill: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub deprecated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum PublishEntry {
    Dependency(String),
    Object(PublishObject),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    #[serde(default)]
    pub dependencies: HashMap<String, DependencyEntry>,
    #[serde(default)]
    pub publish: Vec<PublishEntry>,
}

impl Manifest {
    pub fn new() -> Self {
        Self {
            dependencies: HashMap::new(),
            publish: Vec::new(),
        }
    }

    pub fn load(path: &Path) -> Result<Self, ManifestNotFound> {
        let content = fs::read_to_string(path).map_err(|_| ManifestNotFound {
            message: format!(
                "No skills.json found at {}. Run 'kt init .' to create one.",
                path.display()
            ),
        })?;

        Self::parse_str(&content, path)
    }

    pub fn parse_str(content: &str, path: &Path) -> Result<Self, ManifestNotFound> {
        let manifest: Manifest = serde_json::from_str(content).map_err(|e| ManifestNotFound {
            message: format!("Invalid skills.json at {}: {}", path.display(), e),
        })?;

        manifest.validate().map_err(|e| ManifestNotFound {
            message: e.to_string(),
        })?;

        Ok(manifest)
    }

    pub fn validate(&self) -> Result<(), Box<dyn std::error::Error>> {
        let name_re = regex::Regex::new(r"^[a-zA-Z0-9_-]+$").unwrap();

        for name in self.dependencies.keys() {
            if !name_re.is_match(name) {
                return Err(ManifestInvalidName {
                    message: format!("Invalid skill name '{}': must match [a-zA-Z0-9_-]+", name),
                }
                .into());
            }
        }

        let mut seen = std::collections::HashSet::new();
        for name in self.dependencies.keys() {
            if !seen.insert(name.clone()) {
                return Err(ManifestDuplicateName {
                    message: format!("Duplicate skill name: '{}'", name),
                }
                .into());
            }
        }

        for (name, dependency) in &self.dependencies {
            let has_repo = dependency
                .repo
                .as_deref()
                .is_some_and(|repo| !repo.is_empty());
            let has_path = dependency
                .path
                .as_deref()
                .is_some_and(|path| !path.is_empty());
            if has_repo == has_path {
                return Err(ManifestInvalidName {
                    message: format!(
                        "Dependency '{}' must declare exactly one of repo or path",
                        name
                    ),
                }
                .into());
            }
            if dependency.rev.is_some() && !has_repo {
                return Err(ManifestInvalidName {
                    message: format!("Dependency '{}' cannot use rev with a local path", name),
                }
                .into());
            }
            if let Some(rev) = dependency.rev.as_deref() {
                if parse_rev(rev).is_none() {
                    return Err(ManifestInvalidName {
                        message: format!(
                            "Dependency '{}' has invalid rev '{}'; use commit:<sha>, branch:<name>, or tag:<name>",
                            name, rev
                        ),
                    }
                    .into());
                }
            }
        }

        for entry in &self.publish {
            match entry {
                PublishEntry::Dependency(name) => {
                    if !name_re.is_match(name) {
                        return Err(ManifestInvalidName {
                            message: format!(
                                "Invalid published skill name '{}': must match [a-zA-Z0-9_-]+",
                                name
                            ),
                        }
                        .into());
                    }
                    let dependency = self.dependencies.get(name).ok_or_else(|| {
                        ManifestInvalidName {
                            message: format!(
                                "Published skill '{}' must match a dependency or use an object publish entry with a path",
                                name
                            ),
                        }
                    })?;
                    if dependency
                        .path
                        .as_deref()
                        .is_none_or(|path| path.is_empty())
                    {
                        return Err(ManifestInvalidName {
                            message: format!(
                                "Published skill '{}' must reference a local path dependency",
                                name
                            ),
                        }
                        .into());
                    }
                }
                PublishEntry::Object(object) => {
                    if !name_re.is_match(&object.skill) {
                        return Err(ManifestInvalidName {
                            message: format!(
                                "Invalid published skill name '{}': must match [a-zA-Z0-9_-]+",
                                object.skill
                            ),
                        }
                        .into());
                    }
                    if object.path.trim().is_empty() {
                        return Err(ManifestInvalidName {
                            message: format!(
                                "Published skill '{}' must declare a non-empty path",
                                object.skill
                            ),
                        }
                        .into());
                    }
                }
            }
        }

        Ok(())
    }

    pub fn save(&self, path: &Path) -> Result<(), std::io::Error> {
        let content = serde_json::to_string_pretty(self)?;
        fs::write(path, content)
    }

    pub fn add_remote_dependency(&mut self, name: String, repo: String, rev: Option<String>) {
        self.dependencies.insert(
            name,
            DependencyEntry {
                repo: Some(repo),
                path: None,
                rev,
            },
        );
    }

    pub fn add_local_dependency(&mut self, name: String, path: String) {
        self.dependencies.insert(
            name,
            DependencyEntry {
                repo: None,
                path: Some(path),
                rev: None,
            },
        );
    }

    pub fn add_publish_object(&mut self, skill: String, path: String, deprecated: bool) {
        self.publish
            .retain(|entry| entry.skill_name() != skill.as_str());
        self.publish.push(PublishEntry::Object(PublishObject {
            skill,
            path,
            deprecated,
        }));
    }

    pub fn add_publish_dependency(&mut self, name: String) {
        self.publish
            .retain(|entry| entry.skill_name() != name.as_str());
        self.publish.push(PublishEntry::Dependency(name));
    }

    pub fn remove_dependency(&mut self, name: &str) -> bool {
        self.dependencies.remove(name).is_some()
    }

    pub fn has_dependency(&self, name: &str) -> bool {
        self.dependencies.contains_key(name)
    }

    pub fn remove_skill(&mut self, name: &str) -> bool {
        self.remove_dependency(name)
    }

    pub fn has_skill(&self, name: &str) -> bool {
        self.has_dependency(name)
    }
}

impl PublishEntry {
    pub fn skill_name(&self) -> &str {
        match self {
            PublishEntry::Dependency(name) => name,
            PublishEntry::Object(object) => &object.skill,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RevKind {
    Commit,
    Branch,
    Tag,
}

pub fn parse_rev(rev: &str) -> Option<(RevKind, &str)> {
    let (kind, value) = rev.split_once(':')?;
    if value.is_empty() {
        return None;
    }

    match kind {
        "commit" => Some((RevKind::Commit, value)),
        "branch" => Some((RevKind::Branch, value)),
        "tag" => Some((RevKind::Tag, value)),
        _ => None,
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Default for Manifest {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn test_new() {
        let manifest = Manifest::new();
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.publish.is_empty());
    }

    #[test]
    fn test_default() {
        let manifest = Manifest::default();
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.publish.is_empty());
    }

    #[test]
    fn test_add_remote_dependency() {
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency(
            "test-skill".to_string(),
            "https://github.com/test/repo.git".to_string(),
            Some("commit:a1b2".to_string()),
        );
        assert!(manifest.has_dependency("test-skill"));
        assert_eq!(manifest.dependencies.len(), 1);
    }

    #[test]
    fn test_add_local_dependency() {
        let mut manifest = Manifest::new();
        manifest.add_local_dependency(
            "test-skill".to_string(),
            ".agents/skills/test-skill".to_string(),
        );
        assert_eq!(
            manifest.dependencies["test-skill"].path.as_deref(),
            Some(".agents/skills/test-skill")
        );
    }

    #[test]
    fn test_remove_dependency() {
        let mut manifest = Manifest::new();
        manifest.add_local_dependency("test".to_string(), ".agents/skills/test".to_string());
        assert!(manifest.remove_dependency("test"));
        assert!(!manifest.has_dependency("test"));
    }

    #[test]
    fn test_remove_nonexistent() {
        let mut manifest = Manifest::new();
        assert!(!manifest.remove_dependency("nonexistent"));
    }

    #[test]
    fn test_parse_valid() {
        let content = r#"{
            "dependencies": {
                "my-skill": {
                    "repo": "https://github.com/test/repo.git"
                }
            },
            "publish": []
        }"#;
        let path = Path::new("test.json");
        let manifest = Manifest::parse_str(content, path).unwrap();
        assert!(manifest.has_dependency("my-skill"));
    }

    #[test]
    fn test_parse_local_dependency_and_publish_entries() {
        let content = r#"{
            "dependencies": {
                "my-skill": {
                    "path": ".agents/skills/my-skill"
                }
            },
            "publish": [
                "my-skill",
                {"skill": "extra", "path": "skills/extra", "deprecated": true}
            ]
        }"#;
        let path = Path::new("test.json");
        let manifest = Manifest::parse_str(content, path).unwrap();
        assert_eq!(manifest.publish.len(), 2);
        assert_eq!(manifest.publish[0].skill_name(), "my-skill");
        assert_eq!(manifest.publish[1].skill_name(), "extra");
    }

    #[test]
    fn test_parse_empty() {
        let content = r#"{"dependencies": {}, "publish": []}"#;
        let path = Path::new("test.json");
        let manifest = Manifest::parse_str(content, path).unwrap();
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.publish.is_empty());
    }

    #[test]
    fn test_parse_empty_object_defaults_fields_empty() {
        let content = r#"{}"#;
        let path = Path::new("test.json");
        let manifest = Manifest::parse_str(content, path).unwrap();
        assert!(manifest.dependencies.is_empty());
        assert!(manifest.publish.is_empty());
    }

    #[test]
    fn test_parse_invalid_json() {
        let content = r#"{"dependencies": {}"#;
        let path = Path::new("test.json");
        assert!(Manifest::parse_str(content, path).is_err());
    }

    #[test]
    fn test_parse_invalid_name() {
        let content = r#"{"dependencies": {"bad name!": {"repo": "url"}}, "publish": []}"#;
        let path = Path::new("test.json");
        assert!(Manifest::parse_str(content, path).is_err());
    }

    #[test]
    fn test_validate_valid() {
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("valid_name-123".to_string(), "url".to_string(), None);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_validate_invalid_name() {
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("bad name!".to_string(), "url".to_string(), None);
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn test_save_and_load() {
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("test".to_string(), "url".to_string(), None);
        let dir = std::env::temp_dir().join("ktesio_test_manifest");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skills.json");
        manifest.save(&path).unwrap();
        let loaded = Manifest::load(&path).unwrap();
        assert!(loaded.has_dependency("test"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_load_not_found() {
        assert!(Manifest::load(Path::new("/nonexistent")).is_err());
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = std::env::temp_dir().join("ktesio_test_manifest_roundtrip");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("skills.json");

        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("skill1".to_string(), "url1".to_string(), None);
        manifest.add_remote_dependency("skill2".to_string(), "url2".to_string(), None);
        manifest.save(&path).unwrap();

        let loaded = Manifest::load(&path).unwrap();
        assert_eq!(loaded.dependencies.len(), 2);
        assert!(loaded.has_dependency("skill1"));
        assert!(loaded.has_dependency("skill2"));

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_invalid_name_special_chars() {
        let content = r#"{"dependencies": {"bad@name": {"repo": "url"}}, "publish": []}"#;
        let path = Path::new("test.json");
        assert!(Manifest::parse_str(content, path).is_err());
    }

    #[test]
    fn test_parse_invalid_name_spaces() {
        let content = r#"{"dependencies": {"has space": {"repo": "url"}}, "publish": []}"#;
        let path = Path::new("test.json");
        assert!(Manifest::parse_str(content, path).is_err());
    }

    #[test]
    fn test_validate_valid_names() {
        let mut manifest = Manifest::new();
        manifest.add_remote_dependency("valid_name-123".to_string(), "url".to_string(), None);
        manifest.add_remote_dependency("another-skill".to_string(), "url".to_string(), None);
        manifest.add_remote_dependency("skill123".to_string(), "url".to_string(), None);
        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn test_rejects_old_schema_fields() {
        let content = r#"{"skills": {}, "exports": {}}"#;
        assert!(Manifest::parse_str(content, Path::new("test.json")).is_err());
    }

    #[test]
    fn test_rejects_dependency_with_repo_and_path() {
        let content =
            r#"{"dependencies": {"docs": {"repo": "url", "path": "skills/docs"}}, "publish": []}"#;
        assert!(Manifest::parse_str(content, Path::new("test.json")).is_err());
    }

    #[test]
    fn test_rejects_dependency_without_repo_or_path() {
        let content = r#"{"dependencies": {"docs": {}}, "publish": []}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn test_rejects_local_dependency_with_rev() {
        let content = r#"{"dependencies": {"docs": {"path": "skills/docs", "rev": "branch:main"}}, "publish": []}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("cannot use rev"));
    }

    #[test]
    fn test_rejects_dependency_with_invalid_rev() {
        let content =
            r#"{"dependencies": {"docs": {"repo": "url", "rev": "main"}}, "publish": []}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("invalid rev"));
    }

    #[test]
    fn test_rejects_publish_string_without_matching_dependency() {
        let content = r#"{"dependencies": {}, "publish": ["docs"]}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("must match a dependency"));
    }

    #[test]
    fn test_rejects_publish_string_with_invalid_name() {
        let content = r#"{"dependencies": {}, "publish": ["bad name"]}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("Invalid published"));
    }

    #[test]
    fn test_rejects_publish_string_for_remote_dependency() {
        let content = r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": ["docs"]}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("local path dependency"));
    }

    #[test]
    fn test_rejects_publish_string_with_empty_local_dependency_path() {
        let content = r#"{"dependencies": {"docs": {"path": ""}}, "publish": ["docs"]}"#;
        let error = Manifest::parse_str(content, Path::new("test.json")).unwrap_err();

        assert!(error.to_string().contains("exactly one"));
    }

    #[test]
    fn test_rejects_publish_object_invalid_name_and_empty_path() {
        let invalid_name =
            r#"{"dependencies": {}, "publish": [{"skill": "bad name", "path": "skills/docs"}]}"#;
        let empty_path = r#"{"dependencies": {}, "publish": [{"skill": "docs", "path": "  "}]}"#;

        let name_error = Manifest::parse_str(invalid_name, Path::new("test.json")).unwrap_err();
        let path_error = Manifest::parse_str(empty_path, Path::new("test.json")).unwrap_err();

        assert!(name_error.to_string().contains("Invalid published"));
        assert!(path_error.to_string().contains("non-empty path"));
    }

    #[test]
    fn test_parse_rev() {
        assert_eq!(parse_rev("commit:abc").unwrap().0, RevKind::Commit);
        assert_eq!(parse_rev("branch:main").unwrap().0, RevKind::Branch);
        assert_eq!(parse_rev("tag:v1").unwrap().0, RevKind::Tag);
        assert!(parse_rev("main").is_none());
        assert!(parse_rev("commit:").is_none());
        assert!(parse_rev("other:value").is_none());
    }
}
