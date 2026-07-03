use std::path::{Path, PathBuf};

use dialoguer::{Confirm, MultiSelect, Select};
use indicatif::{MultiProgress, ProgressBar};

use crate::discovery::{self, DiscoveredSkill, SkillType};
use crate::error::{InstallAlreadyExists, InstallInvalidFormat, SkillCopyFailed};
use crate::git;
use crate::install_target;
use crate::lockfile::{LockEntry, Lockfile};
use crate::manifest::{parse_rev, Manifest, PublishEntry, RevKind};
use crate::skill;
use crate::ui;

const LOCAL_COMMIT: &str = "0000000000000000000000000000000000000000";
const FALLBACK_SKILLS_LOCATIONS: &str = "skills/, SKILLS/, or .agents/skills/";

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    pub all: bool,
    pub yes: bool,
    pub no_input: bool,
    pub ssh: bool,
    pub skill: Option<String>,
}

#[cfg(not(tarpaulin_include))]
#[allow(dead_code)]
pub fn run(target: Option<&str>) -> Result<(), Box<dyn std::error::Error>> {
    run_with_options(target, InstallOptions::default())
}

#[cfg(not(tarpaulin_include))]
pub fn run_with_options(
    target: Option<&str>,
    options: InstallOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let project_root = std::env::current_dir()?;
    run_in_with_options(&project_root, target, options)
}

#[allow(dead_code)]
pub(crate) fn run_in(
    project_root: &Path,
    target: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    run_in_with_options(project_root, target, InstallOptions::default())
}

pub(crate) fn run_in_with_options(
    project_root: &Path,
    target: Option<&str>,
    options: InstallOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(t) = target {
        return match parse_install_target(t, &options)? {
            InstallTarget::Named {
                name,
                repo,
                source_skill,
            } => run_single(project_root, &name, &repo, source_skill.as_deref()),
            InstallTarget::Repo { repo, source_skill } => {
                let mut options = options;
                options.skill = source_skill;
                run_repo_target(project_root, &repo, options)
            }
        };
    }

    run_bulk(project_root)
}

fn run_bulk(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let manifest_path = project_root.join("skills.json");

    // T009: Check for manifest existence
    if manifest_path.exists() {
        // Existing manifest flow
        return run_bulk_with_manifest(project_root, &manifest_path);
    }

    // T010: Fallback path when manifest not found
    run_bulk_with_fallback(project_root)
}

fn run_bulk_with_manifest(
    project_root: &Path,
    manifest_path: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut manifest = Manifest::load(manifest_path)?;
    let mut lockfile = Lockfile::load(&project_root.join("skills.lock"))?;
    let entries = manifest
        .dependencies
        .iter()
        .map(|(name, entry)| (name.clone(), entry.clone()))
        .collect::<Vec<_>>();

    let mp = MultiProgress::new();
    let mut errors: Vec<String> = Vec::new();
    let mut lockfile_changed = false;
    let mut manifest_changed = false;

    for (name, entry) in entries {
        let skill_dir = git::skill_dir(project_root, &name);
        if skill_dir.exists() && lockfile.contains(&name) {
            ui::warning(format!(
                "Skill {} already installed, skipping",
                ui::skill_name(&name)
            ));
            continue;
        }

        if skill_dir.exists() && entry.repo.is_some() {
            ui::warning(format!(
                "Skill {} already exists on disk but is not locked; run 'kt init .' to adopt it or remove the directory before installing.",
                ui::skill_name(&name)
            ));
            continue;
        }

        if let Some(path) = entry.path.as_deref() {
            match install_local_dependency(project_root, &name, path) {
                Ok(()) => {
                    lockfile.insert(
                        name.clone(),
                        LockEntry {
                            commit: LOCAL_COMMIT.to_string(),
                            repo: path.to_string(),
                            skill: None,
                        },
                    );
                    lockfile_changed = true;
                    ui::success(format!("Installed {}", ui::skill_name(&name)));
                }
                Err(e) => errors.push(format!("Error installing {}: {}", name, e)),
            }
            continue;
        }

        let Some(repo) = entry.repo.as_deref() else {
            errors.push(format!(
                "Error installing {}: dependency must declare repo or path",
                name
            ));
            continue;
        };

        let resolved = resolve_manifest_entry(repo, Some(name.clone()));
        let pb = ui::install_progress(&mp, &name);
        let commit = match install_repo_to_skill_dir(
            project_root,
            &name,
            &resolved.repo,
            resolved.source_skill.as_deref(),
            entry.rev.as_deref(),
            &pb,
        ) {
            Ok(commit) => commit,
            Err(e) => {
                ui::finish_error(&pb, format!("Error installing {}: {}", name, e));
                errors.push(format!("Error installing {}: {}", name, e));
                continue;
            }
        };

        if !skill_dir.exists() {
            ui::finish_error(
                &pb,
                format!("Error installing {}: destination missing", name),
            );
            errors.push(format!("Error installing {}: destination missing", name));
            continue;
        }

        lockfile.insert(
            name.clone(),
            LockEntry {
                commit,
                repo: resolved.repo.clone(),
                skill: resolved.source_skill.clone(),
            },
        );
        lockfile_changed = true;
        if repo != resolved.repo {
            manifest.add_remote_dependency(name.clone(), resolved.repo.clone(), entry.rev.clone());
            manifest_changed = true;
        }

        ui::finish_success(&pb, format!("Installed {}", ui::skill_name(&name)));
    }

    if manifest_changed {
        manifest.save(manifest_path)?;
    }

    if lockfile_changed {
        lockfile.save(&project_root.join("skills.lock"))?;
    }

    if !errors.is_empty() {
        eprintln!();
        ui::error("Errors encountered:");
        for err in &errors {
            eprintln!("  {}", err);
        }
    }

    Ok(())
}

/// T010, T011, T012, T013, T014, T015: Fallback discovery when manifest not found
fn run_bulk_with_fallback(project_root: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // T011: Display warning messages
    ui::warning("No skills.json found. Auto-discovering skills...");
    ui::info("Discovering skills in repository...");

    // T004: Find skills directory
    let skills_dir = match discovery::find_skills_directory(project_root) {
        Some(dir) => dir,
        None => {
            return Err(crate::error::DiscoveryError {
                message: format!(
                    "No fallback skill directory found ({}). Cannot discover skills without a manifest.",
                    FALLBACK_SKILLS_LOCATIONS
                ),
            }
            .into());
        }
    };

    // T005: Discover skills in the directory
    let result = discovery::discover_skills(&skills_dir);

    // Display any warnings from discovery
    for warning in &result.warnings {
        ui::warning(warning);
    }

    // T013: Handle empty skills directory
    if result.skills.is_empty() {
        return Err(crate::error::SkillsDirectoryEmpty {
            message: "No skills found in the discovered directory".to_string(),
        }
        .into());
    }

    // T012, T019: Handle selection based on number of skills
    let selected_skill = if result.skills.len() == 1 {
        // T019: Auto-select when exactly one skill found
        result.skills.into_iter().next().unwrap()
    } else {
        // T012: Prompt user to select from multiple skills
        prompt_user_selection(&result.skills)?
    };

    // T015: Install the selected skill
    install_discovered_skill(project_root, &selected_skill)
}

/// T012: Prompt user to select a skill from discovered options
#[cfg(not(tarpaulin_include))]
fn prompt_user_selection(
    skills: &[DiscoveredSkill],
) -> Result<DiscoveredSkill, Box<dyn std::error::Error>> {
    println!(
        "\n{}\n",
        ui::table_header("Multiple skills found in repository")
    );

    // Display numbered list
    for (i, skill) in skills.iter().enumerate() {
        println!("  {}. {}", i + 1, ui::skill_name(&skill.name));
    }

    // T014: Handle user cancellation
    let selection = Select::new()
        .with_prompt(format!(
            "\nSelect skill to install (1-{}, or 'q' to cancel)",
            skills.len()
        ))
        .items(skills.iter().map(|s| s.name.as_str()).collect::<Vec<_>>())
        .default(0)
        .interact_opt()?;

    match selection {
        Some(index) => Ok(skills[index].clone()),
        None => {
            // User pressed Ctrl+C or Esc
            ui::warning("Installation cancelled");
            std::process::exit(3);
        }
    }
}

#[cfg(tarpaulin_include)]
fn prompt_user_selection(
    _skills: &[DiscoveredSkill],
) -> Result<DiscoveredSkill, Box<dyn std::error::Error>> {
    Err(crate::error::DiscoveryError {
        message: "Interactive selection is disabled during coverage runs".to_string(),
    }
    .into())
}

/// T015: Install a discovered skill
fn install_discovered_skill(
    project_root: &Path,
    skill: &DiscoveredSkill,
) -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    let pb = ui::install_progress(&mp, &skill.name);

    // For discovered skills, we need to clone from a URL
    // Since discovery finds skills in a local directory, we need to handle this differently
    // The skill.path is the local path to the skill file/directory
    // We'll copy it directly instead of cloning

    let skill_dir = git::skill_dir(project_root, &skill.name);
    let source_is_destination =
        skill.skill_type == SkillType::Directory && paths_are_same(&skill.path, &skill_dir)?;
    if !source_is_destination {
        std::fs::create_dir_all(&skill_dir)?;
    }

    pb.set_position(40);
    pb.set_message(format!(
        "Copying discovered skill {}",
        ui::skill_name(&skill.name)
    ));

    if !source_is_destination {
        // Copy the skill from the discovered location
        match skill.skill_type {
            SkillType::File => {
                // Copy the .md file
                std::fs::copy(&skill.path, skill_dir.join(skill.path.file_name().unwrap()))?;
            }
            SkillType::Directory => {
                // Copy the directory contents
                copy_dir_recursive(&skill.path, &skill_dir)?;
            }
        }
    }

    // Create a minimal lockfile entry
    let mut lockfile = Lockfile::load(&project_root.join("skills.lock"))?;
    lockfile.insert(
        skill.name.clone(),
        LockEntry {
            commit: "0".repeat(40), // Local discovery has no git commit to lock.
            repo: skill.path.to_string_lossy().to_string(),
            skill: None,
        },
    );
    lockfile.save(&project_root.join("skills.lock"))?;

    ui::finish_success(&pb, format!("Installed {}", ui::skill_name(&skill.name)));
    ui::success(format!("Installed {}", ui::skill_name(&skill.name)));

    Ok(())
}

fn paths_are_same(left: &Path, right: &Path) -> Result<bool, Box<dyn std::error::Error>> {
    if !left.exists() || !right.exists() {
        return Ok(false);
    }

    Ok(left.canonicalize()? == right.canonicalize()?)
}

/// Copy directory recursively, skipping .git and other common directories
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if !dst.exists() {
        std::fs::create_dir_all(dst)?;
    }

    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        let name = entry.file_name().to_string_lossy().to_string();
        if name == ".git" || name == "target" || name == "node_modules" {
            continue;
        }

        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }

    Ok(())
}

fn copy_installable_path(src: &Path, dst: &Path) -> Result<(), Box<dyn std::error::Error>> {
    if src.is_dir() {
        return copy_dir_recursive(src, dst);
    }

    std::fs::create_dir_all(dst)?;
    let file_name = src.file_name().ok_or_else(|| SkillCopyFailed {
        message: format!("Cannot determine file name for {}", src.display()),
    })?;
    std::fs::copy(src, dst.join(file_name))?;
    Ok(())
}

fn install_local_dependency(
    project_root: &Path,
    name: &str,
    dependency_path: &str,
) -> Result<(), Box<dyn std::error::Error>> {
    let src = project_root.join(dependency_path);
    if !src.exists() {
        return Err(SkillCopyFailed {
            message: format!("Local dependency path '{}' does not exist", dependency_path),
        }
        .into());
    }

    let skill_dir = git::skill_dir(project_root, name);
    if skill_dir.exists() {
        let canonical_skill_dir = skill_dir.canonicalize()?;
        let canonical_src = src.canonicalize()?;
        if canonical_src == canonical_skill_dir || canonical_src.starts_with(&canonical_skill_dir) {
            return Ok(());
        }

        return Err(SkillCopyFailed {
            message: format!(
                "Destination skill directory already exists at {}",
                skill_dir.display()
            ),
        }
        .into());
    }

    copy_installable_path(&src, &skill_dir)
}

fn run_single(
    project_root: &Path,
    name: &str,
    repo_url: &str,
    source_skill: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    if !is_valid_skill_name(name) {
        return Err(InstallInvalidFormat {
            message: format!("Invalid skill name '{}': must match [a-zA-Z0-9_-]+", name),
        }
        .into());
    }

    let manifest_path = project_root.join("skills.json");
    let mut manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };

    if manifest.has_skill(name) {
        return Err(InstallAlreadyExists {
            message: format!("Skill '{}' already exists in manifest, skipping", name),
        }
        .into());
    }

    let mp = MultiProgress::new();
    let pb = ui::install_progress(&mp, name);

    let source_skill = source_skill.unwrap_or(name);
    let commit =
        install_repo_to_skill_dir(project_root, name, repo_url, Some(source_skill), None, &pb)?;

    manifest.add_remote_dependency(name.to_string(), repo_url.to_string(), None);
    manifest.save(&manifest_path)?;

    let mut lockfile = Lockfile::load(&project_root.join("skills.lock"))?;
    lockfile.insert(
        name.to_string(),
        LockEntry {
            commit,
            repo: repo_url.to_string(),
            skill: Some(source_skill.to_string()),
        },
    );
    lockfile.save(&project_root.join("skills.lock"))?;

    ui::finish_success(&pb, format!("Installed {}", ui::skill_name(name)));
    ui::success(format!(
        "Installed {} from {}",
        ui::skill_name(name),
        project_root.display()
    ));

    Ok(())
}

fn install_repo_to_skill_dir(
    project_root: &Path,
    name: &str,
    repo_url: &str,
    source_skill: Option<&str>,
    rev: Option<&str>,
    pb: &ProgressBar,
) -> Result<String, Box<dyn std::error::Error>> {
    let skill_dir = git::skill_dir(project_root, name);
    if skill_dir.exists() {
        return Err(SkillCopyFailed {
            message: format!(
                "Destination skill directory already exists at {}",
                skill_dir.display()
            ),
        }
        .into());
    }

    let workspace = InstallWorkspace::create(project_root, name)?;

    pb.set_message(format!("Cloning {}", ui::skill_name(name)));
    git::clone_with_progress(repo_url, &workspace.clone_dir, pb)?;

    if let Some(rev) = rev {
        checkout_manifest_rev(&workspace.clone_dir, rev)?;
    }

    let commit = git::rev_parse_head(&workspace.clone_dir).unwrap_or_default();

    pb.set_position(92);
    pb.set_message(format!("Copying files for {}", ui::skill_name(name)));
    let prompter = DialoguerFallbackPrompter;
    copy_repo_content_for_install(
        &workspace.clone_dir,
        &workspace.install_dir,
        source_skill,
        &prompter,
    )?;

    if let Some(parent) = skill_dir.parent() {
        std::fs::create_dir_all(parent)?;
    }
    pb.set_position(96);
    pb.set_message(format!("Finalizing {}", ui::skill_name(name)));
    std::fs::rename(&workspace.install_dir, &skill_dir)?;

    Ok(commit)
}

fn checkout_manifest_rev(repo_dir: &Path, rev: &str) -> Result<(), Box<dyn std::error::Error>> {
    let Some((kind, value)) = parse_rev(rev) else {
        return Err(SkillCopyFailed {
            message: format!(
                "Invalid rev '{}'; use commit:<sha>, branch:<name>, or tag:<name>",
                rev
            ),
        }
        .into());
    };

    let checkout_target = match kind {
        RevKind::Commit => value.to_string(),
        RevKind::Branch => format!("origin/{value}"),
        RevKind::Tag => value.to_string(),
    };
    git::checkout_rev(repo_dir, &checkout_target)
}

fn copy_repo_content_for_install(
    clone_dir: &Path,
    dest_dir: &Path,
    source_skill: Option<&str>,
    prompter: &dyn FallbackPrompter,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(source_skill) = source_skill {
        let installables = discover_repo_installables_for_exact(clone_dir)?;
        let installable = installables
            .iter()
            .find(|installable| installable.name == source_skill)
            .ok_or_else(|| SkillCopyFailed {
                message: format!(
                    "Source skill '{}' was not found in the repository",
                    source_skill
                ),
            })?;
        if installable.deprecated {
            ui::warning(format!(
                "Published skill {} is deprecated.",
                ui::skill_name(&installable.name)
            ));
        }
        return copy_installable_path(&installable.path, dest_dir);
    }

    if clone_dir.join("skills.json").exists() {
        return skill::copy_cloned_repo_to_dest(clone_dir, dest_dir);
    }

    if !prompter.confirm_missing_manifest()? {
        return Err(SkillCopyFailed {
            message: "Installation cancelled because source repo has no skills.json".to_string(),
        }
        .into());
    }

    let skills_dir =
        discovery::find_skills_directory(clone_dir).ok_or_else(|| SkillCopyFailed {
            message: format!(
                "Source repo has no skills.json and no fallback skill directory ({})",
                FALLBACK_SKILLS_LOCATIONS
            ),
        })?;
    let skills = discover_fallback_skill_dirs(&skills_dir)?;
    if skills.is_empty() {
        return Err(SkillCopyFailed {
            message: "Source skills directory does not contain skill directories".to_string(),
        }
        .into());
    }

    let selected_indexes = if skills.len() == 1 {
        vec![0]
    } else {
        prompter.select_skill_dirs(&skills)?
    };

    if selected_indexes.is_empty() {
        return Err(SkillCopyFailed {
            message: "No fallback skills selected".to_string(),
        }
        .into());
    }

    for index in selected_indexes {
        let selected = skills.get(index).ok_or_else(|| SkillCopyFailed {
            message: format!("Selected fallback skill index {} is invalid", index),
        })?;
        copy_installable_path(&selected.path, &dest_dir.join(&selected.name))?;
    }

    Ok(())
}

#[derive(Debug, Clone)]
struct FallbackSkillDir {
    name: String,
    path: PathBuf,
}

trait FallbackPrompter {
    fn confirm_missing_manifest(&self) -> Result<bool, Box<dyn std::error::Error>>;
    fn select_skill_dirs(
        &self,
        skills: &[FallbackSkillDir],
    ) -> Result<Vec<usize>, Box<dyn std::error::Error>>;
}

struct DialoguerFallbackPrompter;

#[cfg(not(tarpaulin_include))]
impl FallbackPrompter for DialoguerFallbackPrompter {
    fn confirm_missing_manifest(&self) -> Result<bool, Box<dyn std::error::Error>> {
        ui::warning("Source repo has no skills.json file.");
        Ok(Confirm::new()
            .with_prompt(format!(
                "Fetch skill directories from {} if present?",
                FALLBACK_SKILLS_LOCATIONS
            ))
            .default(false)
            .interact()?)
    }

    fn select_skill_dirs(
        &self,
        skills: &[FallbackSkillDir],
    ) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
        let items = skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>();
        Ok(MultiSelect::new()
            .with_prompt("Select skills to install")
            .items(&items)
            .interact()?)
    }
}

#[cfg(tarpaulin_include)]
impl FallbackPrompter for DialoguerFallbackPrompter {
    fn confirm_missing_manifest(&self) -> Result<bool, Box<dyn std::error::Error>> {
        Ok(false)
    }

    fn select_skill_dirs(
        &self,
        _skills: &[FallbackSkillDir],
    ) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
        Err(SkillCopyFailed {
            message: "Interactive fallback selection is disabled during coverage runs".to_string(),
        }
        .into())
    }
}

fn discover_fallback_skill_dirs(
    skills_dir: &Path,
) -> Result<Vec<FallbackSkillDir>, Box<dyn std::error::Error>> {
    let mut skills = Vec::new();

    for entry in std::fs::read_dir(skills_dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        if file_name.starts_with('.') || file_name == "target" || file_name == "node_modules" {
            continue;
        }
        if !file_type.is_dir() && !file_name.ends_with(".md") {
            continue;
        }

        let name = discovery::normalize_skill_name(&file_name);
        if name.is_empty() {
            continue;
        }

        skills.push(FallbackSkillDir {
            name,
            path: entry.path(),
        });
    }

    skills.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(skills)
}

fn is_valid_skill_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

enum InstallTarget {
    Named {
        name: String,
        repo: String,
        source_skill: Option<String>,
    },
    Repo {
        repo: String,
        source_skill: Option<String>,
    },
}

struct ResolvedManifestEntry {
    repo: String,
    source_skill: Option<String>,
}

fn resolve_manifest_entry(repo: &str, source_skill: Option<String>) -> ResolvedManifestEntry {
    match install_target::resolve_repo_target(repo, false) {
        Ok(resolved) => ResolvedManifestEntry {
            repo: resolved.repo,
            source_skill: source_skill.or(resolved.source_skill),
        },
        Err(_) => ResolvedManifestEntry {
            repo: repo.to_string(),
            source_skill,
        },
    }
}

fn parse_install_target(
    target: &str,
    options: &InstallOptions,
) -> Result<InstallTarget, Box<dyn std::error::Error>> {
    if target.is_empty() {
        return Err(InstallInvalidFormat {
            message: "Install target cannot be empty".to_string(),
        }
        .into());
    }
    if let Some(source_skill) = options.skill.as_deref() {
        if !install_target::is_valid_skill_name(source_skill) {
            return Err(InstallInvalidFormat {
                message: format!(
                    "Invalid source skill '{}': must match [a-zA-Z0-9_-]+",
                    source_skill
                ),
            }
            .into());
        }
    }

    if let Some((name, repo)) = parse_named_target(target)? {
        let resolved = install_target::resolve_repo_target(&repo, options.ssh)?;
        return Ok(InstallTarget::Named {
            name,
            repo: resolved.repo,
            source_skill: options.skill.clone().or(resolved.source_skill),
        });
    }

    let resolved = install_target::resolve_repo_target(target, options.ssh)?;
    Ok(InstallTarget::Repo {
        repo: resolved.repo,
        source_skill: options.skill.clone().or(resolved.source_skill),
    })
}

fn parse_named_target(
    target: &str,
) -> Result<Option<(String, String)>, Box<dyn std::error::Error>> {
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with("ssh://")
        || target.starts_with("git@")
    {
        return Ok(None);
    }

    let Some((name, repo)) = target.split_once(':') else {
        return Ok(None);
    };

    if name.is_empty() || repo.is_empty() || !is_valid_skill_name(name) {
        return Err(InstallInvalidFormat {
            message:
                "Invalid format. Expected name:url (e.g., clap:https://github.com/clap-rs/clap.git)"
                    .to_string(),
        }
        .into());
    }

    Ok(Some((name.to_string(), repo.to_string())))
}

#[derive(Debug, Clone)]
struct SourceInstallable {
    name: String,
    path: PathBuf,
    deprecated: bool,
}

fn discover_repo_installables_for_exact(
    clone_dir: &Path,
) -> Result<Vec<SourceInstallable>, Box<dyn std::error::Error>> {
    let manifest_path = clone_dir.join("skills.json");
    if manifest_path.exists() {
        return discover_manifest_installables(clone_dir, &manifest_path);
    }

    let skills_dir =
        discovery::find_skills_directory(clone_dir).ok_or_else(|| SkillCopyFailed {
            message: format!(
                "Source repo has no skills.json and no fallback skill directory ({})",
                FALLBACK_SKILLS_LOCATIONS
            ),
        })?;
    let fallback = discover_fallback_skill_dirs(&skills_dir)?;
    if fallback.is_empty() {
        return Err(SkillCopyFailed {
            message: "Source skills directory does not contain skill directories".to_string(),
        }
        .into());
    }

    Ok(fallback
        .into_iter()
        .map(|skill| SourceInstallable {
            name: skill.name,
            path: skill.path,
            deprecated: false,
        })
        .collect())
}

fn run_repo_target(
    project_root: &Path,
    repo_url: &str,
    options: InstallOptions,
) -> Result<(), Box<dyn std::error::Error>> {
    let mp = MultiProgress::new();
    let pb = ui::install_progress(&mp, repo_url);
    let workspace = InstallWorkspace::create(project_root, "repo")?;

    pb.set_message("Cloning repository");
    git::clone_with_progress(repo_url, &workspace.clone_dir, &pb)?;
    let commit = git::rev_parse_head(&workspace.clone_dir).unwrap_or_default();

    let prompter = DialoguerFallbackPrompter;
    let installables = if options.skill.is_some() {
        discover_repo_installables_for_exact(&workspace.clone_dir)?
    } else {
        discover_repo_installables(&workspace.clone_dir, options.clone(), &prompter)?
    };
    let selected_indexes = select_repo_installables(&installables, options.clone(), &prompter)?;

    if selected_indexes.is_empty() {
        return Err(SkillCopyFailed {
            message: "No skills selected for installation".to_string(),
        }
        .into());
    }

    let manifest_path = project_root.join("skills.json");
    let mut manifest = if manifest_path.exists() {
        Manifest::load(&manifest_path)?
    } else {
        Manifest::new()
    };
    let mut lockfile = Lockfile::load(&project_root.join("skills.lock"))?;

    let mut staged = Vec::new();
    for index in selected_indexes {
        let installable = installables.get(index).ok_or_else(|| SkillCopyFailed {
            message: format!("Selected skill index {} is invalid", index),
        })?;

        if manifest.has_skill(&installable.name)
            || git::skill_dir(project_root, &installable.name).exists()
        {
            ui::warning(format!(
                "Skill {} already exists, skipping",
                ui::skill_name(&installable.name)
            ));
            continue;
        }

        let staged_dir = workspace.install_dir.join(&installable.name);
        if installable.deprecated {
            ui::warning(format!(
                "Published skill {} is deprecated.",
                ui::skill_name(&installable.name)
            ));
        }
        copy_installable_path(&installable.path, &staged_dir)?;
        staged.push(installable.clone());
    }

    if staged.is_empty() {
        return Err(InstallAlreadyExists {
            message: "No selected skills can be installed because they already exist".to_string(),
        }
        .into());
    }

    if let Some(parent) = git::skill_dir(project_root, "placeholder").parent() {
        std::fs::create_dir_all(parent)?;
    }

    for installable in &staged {
        let dest = git::skill_dir(project_root, &installable.name);
        std::fs::rename(workspace.install_dir.join(&installable.name), &dest)?;
        manifest.add_remote_dependency(installable.name.clone(), repo_url.to_string(), None);
        lockfile.insert(
            installable.name.clone(),
            LockEntry {
                commit: commit.clone(),
                repo: repo_url.to_string(),
                skill: Some(installable.name.clone()),
            },
        );
    }

    manifest.save(&manifest_path)?;
    lockfile.save(&project_root.join("skills.lock"))?;

    ui::finish_success(
        &pb,
        format!(
            "Installed {} skill{}",
            staged.len(),
            if staged.len() == 1 { "" } else { "s" }
        ),
    );

    Ok(())
}

fn discover_repo_installables(
    clone_dir: &Path,
    options: InstallOptions,
    prompter: &dyn FallbackPrompter,
) -> Result<Vec<SourceInstallable>, Box<dyn std::error::Error>> {
    let manifest_path = clone_dir.join("skills.json");
    if manifest_path.exists() {
        return discover_manifest_installables(clone_dir, &manifest_path);
    }

    if options.no_input && !options.yes && !options.all {
        return Err(SkillCopyFailed {
            message: "Source repo has no skills.json and --no-input prevents fallback confirmation"
                .to_string(),
        }
        .into());
    }

    if !options.yes && !options.all && !prompter.confirm_missing_manifest()? {
        return Err(SkillCopyFailed {
            message: "Installation cancelled because source repo has no skills.json".to_string(),
        }
        .into());
    }

    let skills_dir =
        discovery::find_skills_directory(clone_dir).ok_or_else(|| SkillCopyFailed {
            message: format!(
                "Source repo has no skills.json and no fallback skill directory ({})",
                FALLBACK_SKILLS_LOCATIONS
            ),
        })?;
    let fallback = discover_fallback_skill_dirs(&skills_dir)?;
    if fallback.is_empty() {
        return Err(SkillCopyFailed {
            message: "Source skills directory does not contain skill directories".to_string(),
        }
        .into());
    }

    Ok(fallback
        .into_iter()
        .map(|skill| SourceInstallable {
            name: skill.name,
            path: skill.path,
            deprecated: false,
        })
        .collect())
}

fn discover_manifest_installables(
    clone_dir: &Path,
    manifest_path: &Path,
) -> Result<Vec<SourceInstallable>, Box<dyn std::error::Error>> {
    let manifest = Manifest::load(manifest_path)?;
    if manifest.publish.is_empty() {
        return Err(SkillCopyFailed {
            message: "Source skills.json does not declare any published skills".to_string(),
        }
        .into());
    }

    let mut installables = Vec::new();
    for entry in &manifest.publish {
        match entry {
            PublishEntry::Dependency(name) => {
                let dependency =
                    manifest
                        .dependencies
                        .get(name)
                        .ok_or_else(|| SkillCopyFailed {
                            message: format!(
                                "Published skill '{}' does not match a local dependency",
                                name
                            ),
                        })?;
                let path = dependency.path.as_deref().ok_or_else(|| SkillCopyFailed {
                    message: format!("Published skill '{}' is not a local path dependency", name),
                })?;
                installables.push(SourceInstallable {
                    name: name.clone(),
                    path: clone_dir.join(path),
                    deprecated: false,
                });
            }
            PublishEntry::Object(object) => {
                installables.push(SourceInstallable {
                    name: object.skill.clone(),
                    path: clone_dir.join(&object.path),
                    deprecated: object.deprecated,
                });
            }
        }
    }
    installables.sort_by(|a, b| a.name.cmp(&b.name));

    for installable in &installables {
        if !is_valid_skill_name(&installable.name) {
            return Err(SkillCopyFailed {
                message: format!(
                    "Source published skill name '{}' is invalid: must match [a-zA-Z0-9_-]+",
                    installable.name
                ),
            }
            .into());
        }
        if !installable.path.exists() {
            return Err(SkillCopyFailed {
                message: format!(
                    "Source published path '{}' does not exist",
                    installable.path.display()
                ),
            }
            .into());
        }
    }

    Ok(installables)
}

fn select_repo_installables(
    installables: &[SourceInstallable],
    options: InstallOptions,
    prompter: &dyn FallbackPrompter,
) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
    if let Some(source_skill) = options.skill.as_deref() {
        let index = installables
            .iter()
            .position(|installable| installable.name == source_skill)
            .ok_or_else(|| SkillCopyFailed {
                message: format!(
                    "Source skill '{}' was not found in the repository",
                    source_skill
                ),
            })?;
        return Ok(vec![index]);
    }

    if options.all || installables.len() == 1 {
        return Ok((0..installables.len()).collect());
    }

    if options.no_input || options.yes {
        return Err(SkillCopyFailed {
            message: "Multiple skills are available; use --all or run interactively to select"
                .to_string(),
        }
        .into());
    }

    let fallback = installables
        .iter()
        .map(|installable| FallbackSkillDir {
            name: installable.name.clone(),
            path: installable.path.clone(),
        })
        .collect::<Vec<_>>();
    prompter.select_skill_dirs(&fallback)
}

struct InstallWorkspace {
    root: PathBuf,
    clone_dir: PathBuf,
    install_dir: PathBuf,
}

impl InstallWorkspace {
    fn create(project_root: &Path, name: &str) -> Result<Self, std::io::Error> {
        let temp_parent = project_root.join(".agents").join(".tmp");
        std::fs::create_dir_all(&temp_parent)?;

        let safe_name: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                    ch
                } else {
                    '_'
                }
            })
            .collect();

        for attempt in 0..100 {
            let root = temp_parent.join(format!(
                "install-{}-{}-{}",
                safe_name,
                std::process::id(),
                attempt
            ));
            if root.exists() {
                continue;
            }

            std::fs::create_dir(&root)?;
            return Ok(Self {
                clone_dir: root.join("clone"),
                install_dir: root.join("install"),
                root,
            });
        }

        Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            "could not create a unique install workspace",
        ))
    }
}

impl Drop for InstallWorkspace {
    fn drop(&mut self) {
        let temp_parent = self.root.parent().map(Path::to_path_buf);
        let _ = std::fs::remove_dir_all(&self.root);
        if let Some(parent) = temp_parent {
            let _ = std::fs::remove_dir(&parent);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    struct FakeFallbackPrompter {
        confirm: bool,
        selections: Vec<usize>,
    }

    impl FallbackPrompter for FakeFallbackPrompter {
        fn confirm_missing_manifest(&self) -> Result<bool, Box<dyn std::error::Error>> {
            Ok(self.confirm)
        }

        fn select_skill_dirs(
            &self,
            _skills: &[FallbackSkillDir],
        ) -> Result<Vec<usize>, Box<dyn std::error::Error>> {
            Ok(self.selections.clone())
        }
    }

    #[test]
    fn test_run_invalid_format() {
        let dir = std::env::temp_dir().join("ktesio_test_install_invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, Some("invalidformat"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_invalid_format_no_colon() {
        let dir = std::env::temp_dir().join("ktesio_test_install_nocolon");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, Some("nameonly"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_invalid_format_empty_name() {
        let dir = std::env::temp_dir().join("ktesio_test_install_ename");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, Some(":url"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_invalid_format_empty_url() {
        let dir = std::env::temp_dir().join("ktesio_test_install_eurl");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, Some("name:"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_empty() {
        let dir = std::env::temp_dir().join("ktesio_test_install_empty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let result = run_in(&dir, None);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_already_installed() {
        let dir = std::env::temp_dir().join("ktesio_test_install_already");
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
        let result = run_in(&dir, None);
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_clone_fails() {
        let dir = std::env::temp_dir().join("ktesio_test_install_clonefail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing_repo = dir.join("missing-repo");
        std::fs::write(
            dir.join("skills.json"),
            format!(
                r#"{{"dependencies": {{"test": {{"repo": "{}"}}}}, "publish": []}}"#,
                missing_repo.display()
            ),
        )
        .unwrap();
        let result = run_in(&dir, None);
        assert!(result.is_ok());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_already_exists() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_exists");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        let result = run_in(&dir, Some("test:https://example.com/repo.git"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_clone_fails() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_clonefail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let target = format!("test:{}", dir.join("missing-repo").display());
        let result = run_in(&dir, Some(&target));
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_to_string(dir.join("skills.json")).unwrap(),
            r#"{"dependencies": {}, "publish": []}"#
        );
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_invalid_name_does_not_write_metadata() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_invalid_name");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let target = format!("bad name:{}", dir.join("repo").display());
        let result = run_in(&dir, Some(&target));
        assert!(result.is_err());
        assert!(!dir.join("skills.json").exists());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_rejects_invalid_name_directly() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_invalid_name_direct");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = run_single(&dir, "bad name", "url", None);

        assert!(result.is_err());
        assert!(!dir.join("skills.json").exists());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_creates_manifest_when_missing() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_new_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_local_repo(&dir, "source");

        let result = run_single(&dir, "source", repo.to_str().unwrap(), None);

        assert!(result.is_ok());
        assert!(dir.join("skills.json").exists());
        assert!(dir.join("skills.lock").exists());
        assert!(dir.join(".agents/skills/source/SKILL.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_success_with_local_repo() {
        let dir = std::env::temp_dir().join("ktesio_test_install_single_success");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let repo = create_local_repo(&dir, "source");

        let target = format!("source:{}", repo.display());
        let result = run_in(&dir, Some(&target));

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/source").exists());
        assert!(dir.join(".agents/skills/source/SKILL.md").exists());
        assert!(!dir.join(".agents/skills/source/README.md").exists());
        assert!(!dir.join(".agents/skills/source/.git").exists());
        assert!(dir.join("skills.lock").exists());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_in_with_options_repo_target_installs_selected_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_install_repo_target_dispatch");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_multi_local_repo(&dir, "source");

        let result = run_in_with_options(
            &dir,
            Some(repo.to_str().unwrap()),
            InstallOptions {
                skill: Some("beta".to_string()),
                ..InstallOptions::default()
            },
        );

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/beta/SKILL.md").exists());
        assert!(!dir.join(".agents/skills/alpha").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_requires_confirmation_without_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_decline");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/one")).unwrap();
        std::fs::write(src.join("skills/one/SKILL.md"), "# One").unwrap();

        let prompter = FakeFallbackPrompter {
            confirm: false,
            selections: vec![],
        };
        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_err());
        assert!(!dst.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_multiselects_fallback_directories() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_multiselect");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        std::fs::write(src.join("skills/alpha/SKILL.md"), "# Alpha").unwrap();
        std::fs::write(src.join("skills/beta/SKILL.md"), "# Beta").unwrap();
        std::fs::write(src.join("README.md"), "not installed").unwrap();

        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![1],
        };
        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_ok());
        assert!(!dst.join("alpha").exists());
        assert!(dst.join("beta/SKILL.md").exists());
        assert!(!dst.join("README.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_auto_selects_single_fallback_directory() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_single_fallback");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("SKILLS/only")).unwrap();
        std::fs::write(src.join("SKILLS/only/SKILL.md"), "# Only").unwrap();

        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };
        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_ok());
        assert!(dst.join("only/SKILL.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_auto_selects_single_fallback_file() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_single_fallback_file");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills")).unwrap();
        std::fs::write(src.join("skills/file-skill.md"), "# File Skill").unwrap();

        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };
        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_ok());
        assert!(dst.join("file-skill/file-skill.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_uses_manifest_without_prompt() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/docs")).unwrap();
        std::fs::write(src.join("skills/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            src.join("skills.json"),
            r#"{"publish": [{"skill": "docs", "path": "skills/docs"}]}"#,
        )
        .unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: false,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_ok());
        assert!(dst.join("docs/SKILL.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_reports_missing_exact_source_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_missing_exact");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/docs")).unwrap();
        std::fs::write(src.join("skills/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            src.join("skills.json"),
            r#"{"publish": [{"skill": "docs", "path": "skills/docs"}]}"#,
        )
        .unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, Some("missing"), &prompter);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("was not found"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_copies_deprecated_exact_source_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_deprecated_exact");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/docs")).unwrap();
        std::fs::write(src.join("skills/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            src.join("skills.json"),
            r#"{"publish": [{"skill": "docs", "path": "skills/docs", "deprecated": true}]}"#,
        )
        .unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, Some("docs"), &prompter);

        assert!(result.is_ok());
        assert!(dst.join("SKILL.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_local_dependency_copies_directory_file_and_self_path() {
        let dir = std::env::temp_dir().join("ktesio_test_install_local_dependency");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local/docs")).unwrap();
        std::fs::write(dir.join("local/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(dir.join("file-skill.md"), "# File").unwrap();

        let dir_result = install_local_dependency(&dir, "docs", "local/docs");
        let file_result = install_local_dependency(&dir, "file-skill", "file-skill.md");
        let self_result = install_local_dependency(&dir, "docs", ".agents/skills/docs");

        assert!(dir_result.is_ok());
        assert!(file_result.is_ok());
        assert!(self_result.is_ok());
        assert!(dir.join(".agents/skills/docs/SKILL.md").exists());
        assert!(dir.join(".agents/skills/file-skill/file-skill.md").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_local_dependency_reports_missing_and_existing_destination() {
        let dir = std::env::temp_dir().join("ktesio_test_install_local_dependency_errors");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/docs")).unwrap();
        std::fs::create_dir_all(dir.join("local/docs")).unwrap();

        let missing = install_local_dependency(&dir, "missing", "local/missing");
        let existing = install_local_dependency(&dir, "docs", "local/docs");

        assert!(missing.is_err());
        assert!(existing.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_paths_are_same_handles_existing_and_missing_paths() {
        let dir = std::env::temp_dir().join("ktesio_test_paths_are_same");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skill")).unwrap();

        assert!(paths_are_same(&dir.join("skill"), &dir.join("skill")).unwrap());
        assert!(!paths_are_same(&dir.join("skill"), &dir.join("missing")).unwrap());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_installable_path_rejects_missing_file_name() {
        let dir = std::env::temp_dir().join("ktesio_test_copy_installable_no_file_name");
        let _ = std::fs::remove_dir_all(&dir);

        let result = copy_installable_path(Path::new(""), &dir);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Cannot determine"));
    }

    #[test]
    fn test_checkout_manifest_rev_rejects_invalid_rev() {
        let dir = std::env::temp_dir().join("ktesio_test_checkout_invalid_rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let result = checkout_manifest_rev(&dir, "main");

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid rev"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_fallback_no_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_install_fallback_nodir");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_in(&dir, None);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_fallback_empty_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_install_fallback_empty");
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let result = run_in(&dir, None);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_fallback_single_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_install_fallback_single");
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        std::fs::write(dir.join("skills/test-skill.md"), "# Test Skill").unwrap();
        let result = run_in(&dir, None);
        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/test-skill/test-skill.md").exists());
        assert!(dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_dir_recursive_creates_dst() {
        let src = std::env::temp_dir().join("ktesio_test_copy_create_dst_src");
        let dst = std::env::temp_dir().join("ktesio_test_copy_create_dst_dst");
        std::fs::create_dir_all(&src).unwrap();
        std::fs::write(src.join("file.txt"), "content").unwrap();
        let result = copy_dir_recursive(&src, &dst);
        assert!(result.is_ok());
        assert!(dst.join("file.txt").exists());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn test_copy_dir_recursive_nested() {
        let src = std::env::temp_dir().join("ktesio_test_copy_nested_src");
        let dst = std::env::temp_dir().join("ktesio_test_copy_nested_dst");
        std::fs::create_dir_all(src.join("a/b/c")).unwrap();
        std::fs::write(src.join("a/b/c/file.txt"), "deep").unwrap();
        let result = copy_dir_recursive(&src, &dst);
        assert!(result.is_ok());
        assert!(dst.join("a/b/c/file.txt").exists());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn test_copy_dir_recursive_skips_directories() {
        let src = std::env::temp_dir().join("ktesio_test_copy_skip_src");
        let dst = std::env::temp_dir().join("ktesio_test_copy_skip_dst");
        std::fs::create_dir_all(src.join(".git/objects")).unwrap();
        std::fs::create_dir_all(src.join("target/debug")).unwrap();
        std::fs::create_dir_all(src.join("node_modules/pkg")).unwrap();
        std::fs::write(src.join("keep.txt"), "keep").unwrap();
        let result = copy_dir_recursive(&src, &dst);
        assert!(result.is_ok());
        assert!(dst.join("keep.txt").exists());
        assert!(!dst.join(".git").exists());
        assert!(!dst.join("target").exists());
        assert!(!dst.join("node_modules").exists());
        std::fs::remove_dir_all(&src).unwrap();
        std::fs::remove_dir_all(&dst).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_success() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_success");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_empty() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_empty");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {}, "publish": []}"#,
        )
        .unwrap();
        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_invalid() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_invalid");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skills.json"), "not json").unwrap();
        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_fallback_no_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_fallback_nodir");
        std::fs::create_dir_all(&dir).unwrap();
        let result = run_bulk_with_fallback(&dir);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_fallback_empty() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_fallback_empty2");
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let result = run_bulk_with_fallback(&dir);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_already_installed() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_installed");
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
        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));
        assert!(result.is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_skips_existing_unlocked_remote_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_unlocked_remote");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"test": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));

        assert!(result.is_ok());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_installs_local_dependency() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_local_dependency");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("local/docs")).unwrap();
        std::fs::write(dir.join("local/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"path": "local/docs"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/docs/SKILL.md").exists());
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        assert!(lockfile.contains("docs"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_reports_local_dependency_error() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_local_dependency_error");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"path": "local/missing"}}, "publish": []}"#,
        )
        .unwrap();

        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));

        assert!(result.is_ok());
        assert!(!dir.join("skills.lock").exists());
        assert!(!dir.join(".agents/skills/docs").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_installs_local_repo() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_local_repo");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_local_repo(&dir, "source");
        std::fs::write(
            dir.join("skills.json"),
            format!(
                r#"{{"dependencies": {{"source": {{"repo": "{}"}}}}, "publish": []}}"#,
                repo.display()
            ),
        )
        .unwrap();

        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/source/SKILL.md").exists());
        assert!(dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_with_manifest_clone_fails() {
        let dir = std::env::temp_dir().join("ktesio_test_bulk_manifest_clonefail");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let missing_repo = dir.join("missing-repo");
        std::fs::write(
            dir.join("skills.json"),
            format!(
                r#"{{"dependencies": {{"test": {{"repo": "{}"}}}}, "publish": []}}"#,
                missing_repo.display()
            ),
        )
        .unwrap();
        let result = run_bulk_with_manifest(&dir, &dir.join("skills.json"));
        assert!(result.is_ok());
        assert!(!dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_repo_to_skill_dir_rejects_existing_destination() {
        let dir = std::env::temp_dir().join("ktesio_test_install_existing_destination");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(git::skill_dir(&dir, "docs")).unwrap();
        let progress = ProgressBar::hidden();

        let result = install_repo_to_skill_dir(&dir, "docs", "unused", None, None, &progress);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Destination skill directory already exists"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_errors_without_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_no_skills_dir");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(&src).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("no skills.json"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_errors_for_empty_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_empty_skills_dir");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("does not contain skill directories"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_errors_for_empty_selection() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_empty_selection");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("No fallback skills selected"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_copy_repo_content_for_install_errors_for_invalid_selection() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_content_invalid_selection");
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        let dst = dir.join("dst");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![99],
        };

        let result = copy_repo_content_for_install(&src, &dst, None, &prompter);

        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("Selected fallback skill index 99 is invalid"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_bulk_fallback_directory_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_install_fallback_directory_skill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        std::fs::write(dir.join("skills/docs/SKILL.md"), "# Docs").unwrap();

        let result = run_in(&dir, None);

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/docs/SKILL.md").exists());
        assert!(dir.join("skills.lock").exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_fallback_skill_dirs_filters_and_sorts() {
        let dir = std::env::temp_dir().join("ktesio_test_fallback_skill_dir_filter");
        let _ = std::fs::remove_dir_all(&dir);
        let skills = dir.join("skills");
        std::fs::create_dir_all(skills.join("beta")).unwrap();
        std::fs::create_dir_all(skills.join("alpha")).unwrap();
        std::fs::create_dir_all(skills.join(".hidden")).unwrap();
        std::fs::create_dir_all(skills.join("target")).unwrap();
        std::fs::create_dir_all(skills.join("node_modules")).unwrap();
        std::fs::write(skills.join("gamma.md"), "# Gamma").unwrap();
        std::fs::write(skills.join("README.txt"), "not a skill").unwrap();

        let result = discover_fallback_skill_dirs(&skills).unwrap();

        assert_eq!(
            vec!["alpha".to_string(), "beta".to_string(), "gamma".to_string()],
            result
                .into_iter()
                .map(|skill| skill.name)
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_workspace_sanitizes_name() {
        let dir = std::env::temp_dir().join("ktesio_test_install_workspace_sanitize");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let workspace = InstallWorkspace::create(&dir, "bad/name").unwrap();

        assert!(workspace
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("bad_name"));
        drop(workspace);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_install_workspace_errors_after_collisions() {
        let dir = std::env::temp_dir().join("ktesio_test_install_workspace_collisions");
        let _ = std::fs::remove_dir_all(&dir);
        let temp_parent = dir.join(".agents").join(".tmp");
        std::fs::create_dir_all(&temp_parent).unwrap();
        let pid = std::process::id();
        for attempt in 0..100 {
            std::fs::create_dir_all(temp_parent.join(format!("install-full-{}-{}", pid, attempt)))
                .unwrap();
        }

        let result = InstallWorkspace::create(&dir, "full");

        match result {
            Ok(_) => panic!("workspace creation should fail after all names collide"),
            Err(err) => assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists),
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_install_target_recognizes_named_and_repo_targets() {
        let options = InstallOptions::default();
        match parse_install_target("docs:https://example.com/repo.git", &options).unwrap() {
            InstallTarget::Named {
                name,
                repo,
                source_skill,
            } => {
                assert_eq!(name, "docs");
                assert_eq!(repo, "https://example.com/repo.git");
                assert_eq!(source_skill, None);
            }
            InstallTarget::Repo { .. } => panic!("expected named target"),
        }

        match parse_install_target("https://example.com/repo.git", &options).unwrap() {
            InstallTarget::Repo { repo, source_skill } => {
                assert_eq!(repo, "https://example.com/repo.git");
                assert_eq!(source_skill, None);
            }
            InstallTarget::Named { .. } => panic!("expected repo target"),
        }

        match parse_install_target("docs:hashicorp/agent-skills/run-tests", &options).unwrap() {
            InstallTarget::Named {
                name,
                repo,
                source_skill,
            } => {
                assert_eq!(name, "docs");
                assert_eq!(repo, "https://github.com/hashicorp/agent-skills.git");
                assert_eq!(source_skill.as_deref(), Some("run-tests"));
            }
            InstallTarget::Repo { .. } => panic!("expected named target"),
        }

        assert!(parse_install_target("bad name:url", &options).is_err());
        assert!(parse_install_target("name:", &options).is_err());
        assert!(parse_install_target("", &options).is_err());

        let options = InstallOptions {
            ssh: true,
            skill: Some("lint".to_string()),
            ..InstallOptions::default()
        };
        match parse_install_target("hashicorp/agent-skills", &options).unwrap() {
            InstallTarget::Repo { repo, source_skill } => {
                assert_eq!(repo, "git@github.com:hashicorp/agent-skills.git");
                assert_eq!(source_skill.as_deref(), Some("lint"));
            }
            InstallTarget::Named { .. } => panic!("expected repo target"),
        }

        let invalid_skill = InstallOptions {
            skill: Some("bad skill".to_string()),
            ..InstallOptions::default()
        };
        assert!(parse_install_target("hashicorp/agent-skills", &invalid_skill).is_err());
    }

    #[test]
    fn test_resolve_manifest_entry_canonicalizes_github_shorthand() {
        let resolved = resolve_manifest_entry("hashicorp/agent-skills/run-tests", None);

        assert_eq!(
            resolved.repo,
            "https://github.com/hashicorp/agent-skills.git"
        );
        assert_eq!(resolved.source_skill.as_deref(), Some("run-tests"));

        let explicit_skill =
            resolve_manifest_entry("hashicorp/agent-skills/run-tests", Some("lint".to_string()));
        assert_eq!(explicit_skill.source_skill.as_deref(), Some("lint"));
    }

    #[test]
    fn test_discover_repo_installables_from_manifest() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_manifest");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/beta")).unwrap();
        std::fs::create_dir_all(dir.join("skills/alpha")).unwrap();
        std::fs::write(dir.join("skills/alpha/SKILL.md"), "# Alpha").unwrap();
        std::fs::write(dir.join("skills/beta/SKILL.md"), "# Beta").unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"publish": [{"skill": "beta", "path": "skills/beta"}, {"skill": "alpha", "path": "skills/alpha"}]}"#,
        )
        .unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let installables =
            discover_repo_installables(&dir, InstallOptions::default(), &prompter).unwrap();

        assert_eq!(
            vec!["alpha".to_string(), "beta".to_string()],
            installables
                .iter()
                .map(|installable| installable.name.clone())
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_rejects_empty_publish() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_empty");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("skills.json"), r#"{"publish": []}"#).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = discover_repo_installables(&dir, InstallOptions::default(), &prompter);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_rejects_missing_publish_path() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_missing_path");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"publish": [{"skill": "docs", "path": "skills/docs"}]}"#,
        )
        .unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = discover_repo_installables(&dir, InstallOptions::default(), &prompter);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_manifest_installables_rejects_invalid_publish_name() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_invalid_publish");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        std::fs::write(dir.join("skills/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"publish": [{"skill": "bad name", "path": "skills/docs"}]}"#,
        )
        .unwrap();

        let result = discover_manifest_installables(&dir, &dir.join("skills.json"));

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Invalid"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_manifest_installables_accepts_dependency_publish() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_dependency_publish");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        std::fs::write(dir.join("skills/docs/SKILL.md"), "# Docs").unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"docs": {"path": "skills/docs"}}, "publish": ["docs"]}"#,
        )
        .unwrap();

        let installables = discover_manifest_installables(&dir, &dir.join("skills.json")).unwrap();

        assert_eq!(installables.len(), 1);
        assert_eq!(installables[0].name, "docs");
        assert_eq!(installables[0].path, dir.join("skills/docs"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_fallback_skill_dirs_skips_empty_normalized_names() {
        let dir = std::env::temp_dir().join("ktesio_test_fallback_empty_normalized");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("!!!.md"), "# Empty").unwrap();
        std::fs::write(dir.join("docs.md"), "# Docs").unwrap();

        let skills = discover_fallback_skill_dirs(&dir).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "docs");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_resolve_manifest_entry_keeps_unrecognized_repo() {
        let resolved = resolve_manifest_entry("not a shorthand", Some("docs".to_string()));

        assert_eq!(resolved.repo, "not a shorthand");
        assert_eq!(resolved.source_skill.as_deref(), Some("docs"));
    }

    #[test]
    fn test_discover_repo_installables_for_exact_uses_fallback_dirs() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_exact_fallback");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/beta")).unwrap();
        std::fs::write(dir.join("skills/beta/SKILL.md"), "# Beta").unwrap();
        std::fs::write(dir.join("skills/alpha.md"), "# Alpha").unwrap();

        let installables = discover_repo_installables_for_exact(&dir).unwrap();

        assert_eq!(
            vec!["alpha".to_string(), "beta".to_string()],
            installables
                .iter()
                .map(|installable| installable.name.clone())
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_for_exact_reports_fallback_errors() {
        let missing = std::env::temp_dir().join("ktesio_test_repo_installables_exact_missing");
        let empty = std::env::temp_dir().join("ktesio_test_repo_installables_exact_empty");
        let _ = std::fs::remove_dir_all(&missing);
        let _ = std::fs::remove_dir_all(&empty);
        std::fs::create_dir_all(&missing).unwrap();
        std::fs::create_dir_all(empty.join("skills")).unwrap();

        assert!(discover_repo_installables_for_exact(&missing).is_err());
        assert!(discover_repo_installables_for_exact(&empty).is_err());

        std::fs::remove_dir_all(&missing).unwrap();
        std::fs::remove_dir_all(&empty).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_fallback_respects_no_input() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_no_input");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = discover_repo_installables(
            &dir,
            InstallOptions {
                no_input: true,
                ..InstallOptions::default()
            },
            &prompter,
        );

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_fallback_requires_confirmation() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_declined");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/docs")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: false,
            selections: vec![],
        };

        let result = discover_repo_installables(&dir, InstallOptions::default(), &prompter);

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_fallback_reports_missing_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_no_skills_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = discover_repo_installables(
            &dir,
            InstallOptions {
                yes: true,
                ..InstallOptions::default()
            },
            &prompter,
        );

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_fallback_reports_empty_skills_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_empty_skills_dir");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills")).unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let result = discover_repo_installables(
            &dir,
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
            &prompter,
        );

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_discover_repo_installables_fallback_returns_safe_names() {
        let dir = std::env::temp_dir().join("ktesio_test_repo_installables_fallback_names");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("skills/Fancy Skill")).unwrap();
        std::fs::write(dir.join("skills/alpha_beta.md"), "# Alpha").unwrap();
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![],
        };

        let installables = discover_repo_installables(
            &dir,
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
            &prompter,
        )
        .unwrap();

        assert_eq!(
            vec!["Fancy-Skill".to_string(), "alpha_beta".to_string()],
            installables
                .iter()
                .map(|installable| installable.name.clone())
                .collect::<Vec<_>>()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_select_repo_installables_handles_all_single_and_ambiguous() {
        let installables = vec![
            SourceInstallable {
                name: "alpha".to_string(),
                path: PathBuf::from("alpha"),
                deprecated: false,
            },
            SourceInstallable {
                name: "beta".to_string(),
                path: PathBuf::from("beta"),
                deprecated: false,
            },
        ];
        let prompter = FakeFallbackPrompter {
            confirm: true,
            selections: vec![1],
        };

        let all = select_repo_installables(
            &installables,
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
            &prompter,
        )
        .unwrap();
        assert_eq!(all, vec![0, 1]);

        let selected =
            select_repo_installables(&installables, InstallOptions::default(), &prompter).unwrap();
        assert_eq!(selected, vec![1]);

        let no_input = select_repo_installables(
            &installables,
            InstallOptions {
                no_input: true,
                ..InstallOptions::default()
            },
            &prompter,
        );
        assert!(no_input.is_err());

        let yes = select_repo_installables(
            &installables,
            InstallOptions {
                yes: true,
                ..InstallOptions::default()
            },
            &prompter,
        );
        assert!(yes.is_err());

        let single =
            select_repo_installables(&installables[0..1], InstallOptions::default(), &prompter)
                .unwrap();
        assert_eq!(single, vec![0]);

        let missing_skill = select_repo_installables(
            &installables,
            InstallOptions {
                skill: Some("missing".to_string()),
                ..InstallOptions::default()
            },
            &prompter,
        );
        assert!(missing_skill.is_err());
    }

    #[test]
    fn test_run_repo_target_installs_all_published_skills_from_local_repo() {
        let dir = std::env::temp_dir().join("ktesio_test_run_repo_target_all");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_local_repo(&dir, "source");

        let result = run_repo_target(
            &dir,
            repo.to_str().unwrap(),
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
        );

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/source/SKILL.md").exists());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.has_skill("source"));
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        assert!(lockfile.contains("source"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_repo_target_installs_deprecated_published_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_run_repo_target_deprecated");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_deprecated_local_repo(&dir, "legacy");

        let result = run_repo_target(
            &dir,
            repo.to_str().unwrap(),
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
        );

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/legacy/SKILL.md").exists());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.has_skill("legacy"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_repo_target_installs_exact_source_skill() {
        let dir = std::env::temp_dir().join("ktesio_test_run_repo_target_exact_skill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_multi_local_repo(&dir, "source");

        let result = run_repo_target(
            &dir,
            repo.to_str().unwrap(),
            InstallOptions {
                skill: Some("beta".to_string()),
                ..InstallOptions::default()
            },
        );

        assert!(result.is_ok());
        assert!(!dir.join(".agents/skills/alpha").exists());
        assert!(dir.join(".agents/skills/beta/SKILL.md").exists());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.dependencies.contains_key("beta"));
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        assert_eq!(
            lockfile.entry("beta").unwrap().skill.as_deref(),
            Some("beta")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_single_installs_exact_source_skill_into_named_destination() {
        let dir = std::env::temp_dir().join("ktesio_test_run_single_exact_skill");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let repo = create_multi_local_repo(&dir, "source");

        let result = run_single(&dir, "docs", repo.to_str().unwrap(), Some("beta"));

        assert!(result.is_ok());
        assert!(dir.join(".agents/skills/docs/SKILL.md").exists());
        assert!(!dir.join(".agents/skills/docs/beta/SKILL.md").exists());
        let manifest = Manifest::load(&dir.join("skills.json")).unwrap();
        assert!(manifest.dependencies.contains_key("docs"));
        let lockfile = Lockfile::load(&dir.join("skills.lock")).unwrap();
        assert_eq!(
            lockfile.entry("docs").unwrap().skill.as_deref(),
            Some("beta")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_run_repo_target_skips_existing_and_errors_when_nothing_installable() {
        let dir = std::env::temp_dir().join("ktesio_test_run_repo_target_existing");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".agents/skills/source")).unwrap();
        std::fs::write(
            dir.join("skills.json"),
            r#"{"dependencies": {"source": {"repo": "url"}}, "publish": []}"#,
        )
        .unwrap();
        let repo = create_local_repo(&dir, "source");

        let result = run_repo_target(
            &dir,
            repo.to_str().unwrap(),
            InstallOptions {
                all: true,
                ..InstallOptions::default()
            },
        );

        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_repo_to_skill_dir_checks_out_commit_rev() {
        let dir = std::env::temp_dir().join("ktesio_test_install_commit_rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (repo, base_commit) = create_rev_repo(&dir);
        let progress = ProgressBar::hidden();

        let result = install_repo_to_skill_dir(
            &dir,
            "source",
            repo.to_str().unwrap(),
            Some("source"),
            Some(&format!("commit:{base_commit}")),
            &progress,
        );

        assert!(result.is_ok());
        let content = std::fs::read_to_string(dir.join(".agents/skills/source/SKILL.md")).unwrap();
        assert!(content.contains("Base"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_repo_to_skill_dir_checks_out_branch_rev() {
        let dir = std::env::temp_dir().join("ktesio_test_install_branch_rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (repo, _) = create_rev_repo(&dir);
        let progress = ProgressBar::hidden();

        let result = install_repo_to_skill_dir(
            &dir,
            "source",
            repo.to_str().unwrap(),
            Some("source"),
            Some("branch:feature"),
            &progress,
        );

        assert!(result.is_ok());
        let content = std::fs::read_to_string(dir.join(".agents/skills/source/SKILL.md")).unwrap();
        assert!(content.contains("Feature"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_install_repo_to_skill_dir_checks_out_tag_rev() {
        let dir = std::env::temp_dir().join("ktesio_test_install_tag_rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let (repo, _) = create_rev_repo(&dir);
        let progress = ProgressBar::hidden();

        let result = install_repo_to_skill_dir(
            &dir,
            "source",
            repo.to_str().unwrap(),
            Some("source"),
            Some("tag:v1"),
            &progress,
        );

        assert!(result.is_ok());
        let content = std::fs::read_to_string(dir.join(".agents/skills/source/SKILL.md")).unwrap();
        assert!(content.contains("Tagged"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    fn create_local_repo(root: &Path, name: &str) -> std::path::PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(repo.join("skills").join(name)).unwrap();
        std::fs::write(repo.join("skills").join(name).join("SKILL.md"), "# Test").unwrap();
        std::fs::write(repo.join("README.md"), "not published").unwrap();
        std::fs::write(
            repo.join("skills.json"),
            format!(
                r#"{{"dependencies": {{}}, "publish": [{{"skill": "{}", "path": "skills/{}"}}]}}"#,
                name, name
            ),
        )
        .unwrap();
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

    fn create_rev_repo(root: &Path) -> (std::path::PathBuf, String) {
        let repo = root.join("rev-source");
        std::fs::create_dir_all(repo.join("skills/source")).unwrap();
        std::fs::write(repo.join("skills/source/SKILL.md"), "# Base").unwrap();
        std::fs::write(
            repo.join("skills.json"),
            r#"{"dependencies": {}, "publish": [{"skill": "source", "path": "skills/source"}]}"#,
        )
        .unwrap();
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
                "base",
            ],
        );
        let base_commit = git::rev_parse_head(&repo).unwrap();

        std::fs::write(repo.join("skills/source/SKILL.md"), "# Tagged").unwrap();
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
                "tagged",
            ],
        );
        run_git(&repo, &["tag", "v1"]);

        run_git(&repo, &["checkout", "-b", "feature"]);
        std::fs::write(repo.join("skills/source/SKILL.md"), "# Feature").unwrap();
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
                "feature",
            ],
        );
        (repo, base_commit)
    }

    fn create_multi_local_repo(root: &Path, name: &str) -> std::path::PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(repo.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(repo.join("skills/beta")).unwrap();
        std::fs::write(repo.join("skills/alpha/SKILL.md"), "# Alpha").unwrap();
        std::fs::write(repo.join("skills/beta/SKILL.md"), "# Beta").unwrap();
        std::fs::write(
            repo.join("skills.json"),
            r#"{"dependencies": {}, "publish": [{"skill": "alpha", "path": "skills/alpha"}, {"skill": "beta", "path": "skills/beta"}]}"#,
        )
        .unwrap();
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

    fn create_deprecated_local_repo(root: &Path, name: &str) -> std::path::PathBuf {
        let repo = root.join(name);
        std::fs::create_dir_all(repo.join("skills").join(name)).unwrap();
        std::fs::write(repo.join("skills").join(name).join("SKILL.md"), "# Legacy").unwrap();
        std::fs::write(
            repo.join("skills.json"),
            format!(
                r#"{{"publish": [{{"skill": "{}", "path": "skills/{}", "deprecated": true}}]}}"#,
                name, name
            ),
        )
        .unwrap();
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
                "deprecated fixture",
            ],
        );
        repo
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
