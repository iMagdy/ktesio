use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::error::{GitCheckoutFailed, GitCloneFailed, GitFetchFailed, GitRevParseFailed};
use indicatif::ProgressBar;

pub fn is_git_available() -> bool {
    Command::new("git")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(test)]
pub fn clone(url: &str, dest: &Path) -> Result<(), Box<dyn std::error::Error>> {
    clone_inner(url, dest, None)
}

pub fn clone_with_progress(
    url: &str,
    dest: &Path,
    progress: &ProgressBar,
) -> Result<(), Box<dyn std::error::Error>> {
    progress.set_position(5);
    progress.set_message("Cloning repository");
    clone_inner(url, dest, Some(progress))?;
    progress.set_position(90);
    progress.set_message("Clone complete");
    Ok(())
}

fn clone_inner(
    url: &str,
    dest: &Path,
    progress: Option<&ProgressBar>,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut child = Command::new("git")
        .arg("clone")
        .arg("--progress")
        .arg(url)
        .arg(dest)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| GitCloneFailed {
            message: format!("Failed to run git clone: {}", e),
        })?;

    let mut stderr = child.stderr.take().ok_or_else(|| GitCloneFailed {
        message: "Failed to capture git clone progress".to_string(),
    })?;
    let mut stderr_output = Vec::new();
    let mut progress_buffer = String::new();
    let mut buffer = [0_u8; 4096];

    loop {
        let bytes_read =
            std::io::Read::read(&mut stderr, &mut buffer).map_err(|e| GitCloneFailed {
                message: format!("Failed to read git clone output: {}", e),
            })?;
        if bytes_read == 0 {
            break;
        }

        stderr_output.extend_from_slice(&buffer[..bytes_read]);
        if let Some(progress) = progress {
            observe_progress(progress, &buffer[..bytes_read], &mut progress_buffer);
        }
    }

    let status = child.wait().map_err(|e| GitCloneFailed {
        message: format!("Failed to wait for git clone: {}", e),
    })?;

    if !status.success() {
        return Err(GitCloneFailed {
            message: format!(
                "git clone failed for {}: {}",
                url,
                summarize_git_failure(&stderr_output)
            ),
        }
        .into());
    }

    Ok(())
}

pub fn fetch(repo_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("fetch")
        .arg("origin")
        .output()
        .map_err(|e| GitFetchFailed {
            message: format!("Failed to run git fetch: {}", e),
        })?;

    if !output.status.success() {
        return Err(GitFetchFailed {
            message: format!(
                "git fetch failed in {}: {}",
                repo_dir.display(),
                summarize_git_failure(&output.stderr)
            ),
        }
        .into());
    }

    Ok(())
}

pub fn checkout_default_branch(repo_dir: &Path) -> Result<(), Box<dyn std::error::Error>> {
    let default_branch = resolve_default_branch(repo_dir)?;

    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("checkout")
        .arg(format!("origin/{}", default_branch))
        .output()
        .map_err(|e| GitCheckoutFailed {
            message: format!("Failed to run git checkout: {}", e),
        })?;

    if !output.status.success() {
        return Err(GitCheckoutFailed {
            message: format!(
                "git checkout origin/{} failed in {}: {}",
                default_branch,
                repo_dir.display(),
                summarize_git_failure(&output.stderr)
            ),
        }
        .into());
    }

    Ok(())
}

pub fn checkout_rev(repo_dir: &Path, rev: &str) -> Result<(), Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("checkout")
        .arg(rev)
        .output()
        .map_err(|e| GitCheckoutFailed {
            message: format!("Failed to run git checkout: {}", e),
        })?;

    if !output.status.success() {
        return Err(GitCheckoutFailed {
            message: format!(
                "git checkout {} failed in {}: {}",
                rev,
                repo_dir.display(),
                summarize_git_failure(&output.stderr)
            ),
        }
        .into());
    }

    Ok(())
}

pub fn rev_parse_head(repo_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("rev-parse")
        .arg("HEAD")
        .output()
        .map_err(|e| GitRevParseFailed {
            message: format!("Failed to run git rev-parse: {}", e),
        })?;

    if !output.status.success() {
        return Err(GitRevParseFailed {
            message: format!("git rev-parse HEAD failed in {}", repo_dir.display()),
        }
        .into());
    }

    let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Ok(sha)
}

pub fn resolve_default_branch(repo_dir: &Path) -> Result<String, Box<dyn std::error::Error>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_dir)
        .arg("symbolic-ref")
        .arg("refs/remotes/origin/HEAD")
        .output()
        .map_err(|e| GitRevParseFailed {
            message: format!("Failed to resolve default branch: {}", e),
        })?;

    if output.status.success() {
        let refname = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // refs/remotes/origin/main -> main
        if let Some(branch) = refname.strip_prefix("refs/remotes/origin/") {
            return Ok(branch.to_string());
        }
    }

    // Fallback: check common branch names
    for branch in &["main", "master"] {
        let check = Command::new("git")
            .arg("-C")
            .arg(repo_dir)
            .arg("rev-parse")
            .arg("--verify")
            .arg(format!("refs/heads/{}", branch))
            .output();

        if let Ok(out) = check {
            if out.status.success() {
                return Ok(branch.to_string());
            }
        }
    }

    Ok("main".to_string())
}

pub fn skill_dir(project_root: &Path, name: &str) -> PathBuf {
    project_root.join(".agents").join("skills").join(name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GitProgressStage {
    Counting,
    Compressing,
    Receiving,
    Resolving,
    Updating,
}

impl GitProgressStage {
    fn label(self) -> &'static str {
        match self {
            GitProgressStage::Counting => "Counting objects",
            GitProgressStage::Compressing => "Compressing objects",
            GitProgressStage::Receiving => "Receiving objects",
            GitProgressStage::Resolving => "Resolving deltas",
            GitProgressStage::Updating => "Writing files",
        }
    }

    fn mapped_position(self, percent: u64) -> u64 {
        let percent = percent.min(100);
        match self {
            GitProgressStage::Counting => 5 + (percent * 15 / 100),
            GitProgressStage::Compressing => 20 + (percent * 20 / 100),
            GitProgressStage::Receiving => 40 + (percent * 35 / 100),
            GitProgressStage::Resolving => 75 + (percent * 15 / 100),
            GitProgressStage::Updating => 90 + (percent * 5 / 100),
        }
    }
}

fn observe_progress(progress: &ProgressBar, bytes: &[u8], progress_buffer: &mut String) {
    progress_buffer.push_str(&String::from_utf8_lossy(bytes));

    while let Some(index) = progress_buffer.find(['\r', '\n']) {
        let line: String = progress_buffer.drain(..=index).collect();
        update_progress_from_line(progress, &line);
    }

    if progress_buffer.len() > 4096 {
        update_progress_from_line(progress, progress_buffer);
        progress_buffer.clear();
    }
}

fn update_progress_from_line(progress: &ProgressBar, line: &str) {
    if let Some((stage, percent)) = parse_git_progress(line) {
        progress.set_position(stage.mapped_position(percent));
        progress.set_message(format!("{} {}%", stage.label(), percent));
    }
}

fn parse_git_progress(line: &str) -> Option<(GitProgressStage, u64)> {
    let cleaned = line
        .trim_matches(|ch| ch == '\r' || ch == '\n')
        .trim()
        .strip_prefix("remote:")
        .map(str::trim)
        .unwrap_or_else(|| line.trim());

    let stage = if cleaned.starts_with("Counting objects:") {
        GitProgressStage::Counting
    } else if cleaned.starts_with("Compressing objects:") {
        GitProgressStage::Compressing
    } else if cleaned.starts_with("Receiving objects:") {
        GitProgressStage::Receiving
    } else if cleaned.starts_with("Resolving deltas:") {
        GitProgressStage::Resolving
    } else if cleaned.starts_with("Updating files:") {
        GitProgressStage::Updating
    } else {
        return None;
    };

    let percent = parse_percent(cleaned)?;
    Some((stage, percent))
}

fn parse_percent(line: &str) -> Option<u64> {
    let percent_index = line.find('%')?;
    let digits_reversed: String = line[..percent_index]
        .chars()
        .rev()
        .skip_while(|ch| ch.is_whitespace())
        .take_while(|ch| ch.is_ascii_digit())
        .collect();

    if digits_reversed.is_empty() {
        return None;
    }

    digits_reversed
        .chars()
        .rev()
        .collect::<String>()
        .parse()
        .ok()
}

fn summarize_git_failure(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).replace('\r', "\n");
    let lines = text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .filter(|line| parse_git_progress(line).is_none())
        .filter(|line| !line.starts_with("Cloning into "))
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    lines
        .last()
        .cloned()
        .unwrap_or_else(|| "git exited without a detailed error".to_string())
}

#[cfg(test)]
pub fn is_installed(project_root: &Path, name: &str) -> bool {
    skill_dir(project_root, name).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_available() {
        assert!(is_git_available());
    }

    #[test]
    fn test_skill_dir() {
        let root = Path::new("/project");
        let dir = skill_dir(root, "my-skill");
        assert_eq!(dir, PathBuf::from("/project/.agents/skills/my-skill"));
    }

    #[test]
    fn test_is_installed() {
        let dir = std::env::temp_dir().join("ktesio_test_is_installed");
        std::fs::create_dir_all(dir.join(".agents/skills/test")).unwrap();
        assert!(is_installed(&dir, "test"));
        assert!(!is_installed(&dir, "other"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_clone_invalid_url() {
        let dir = std::env::temp_dir().join("ktesio_test_clone_invalid");
        let result = clone("/definitely/not/a/ktesio/repo", &dir);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clone_fetch_checkout_and_rev_parse_success() {
        let root = std::env::temp_dir().join("ktesio_test_git_success_paths");
        let _ = std::fs::remove_dir_all(&root);
        let repo = root.join("repo");
        let clone_dir = root.join("clone");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("README.md"), "content").unwrap();
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
                "initial",
            ],
        );

        let progress = ProgressBar::hidden();
        let result = clone_with_progress(repo.to_str().unwrap(), &clone_dir, &progress);

        assert!(result.is_ok());
        assert_eq!(progress.position(), 90);
        assert_eq!(rev_parse_head(&clone_dir).unwrap().len(), 40);
        assert_eq!(resolve_default_branch(&clone_dir).unwrap(), "main");
        assert!(checkout_rev(&clone_dir, "HEAD").is_ok());
        assert!(checkout_default_branch(&clone_dir).is_ok());
        assert!(fetch(&clone_dir).is_ok());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn test_fetch_invalid_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_fetch_invalid");
        let result = fetch(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_rev_parse_head_invalid_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_revparse_invalid");
        let result = rev_parse_head(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_default_branch_invalid_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_resolve_invalid");
        let result = resolve_default_branch(&dir);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "main");
    }

    #[test]
    fn test_resolve_default_branch_falls_back_to_local_main() {
        let dir = std::env::temp_dir().join("ktesio_test_resolve_local_main");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-b", "main"]);

        let result = resolve_default_branch(&dir);

        assert_eq!(result.unwrap(), "main");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_checkout_default_branch_invalid_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_checkout_invalid");
        let result = checkout_default_branch(&dir);
        assert!(result.is_err());
    }

    #[test]
    fn test_checkout_rev_reports_bad_revision() {
        let dir = std::env::temp_dir().join("ktesio_test_checkout_bad_rev");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&dir, &["init", "-b", "main"]);

        let result = checkout_rev(&dir, "not-a-real-revision");

        assert!(result.is_err());
        let message = result.unwrap_err().to_string();
        assert!(message.contains("not-a-real-revision"));
        assert!(message.contains("git checkout"));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_clone_to_existing_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_clone_existing");
        std::fs::create_dir_all(&dir).unwrap();
        let result = clone("/definitely/not/a/ktesio/repo", &dir);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_fetch_non_git_dir() {
        let dir = std::env::temp_dir().join("ktesio_test_fetch_non_git");
        std::fs::create_dir_all(&dir).unwrap();
        let result = fetch(&dir);
        assert!(result.is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn test_parse_git_progress_from_remote_line() {
        let parsed = parse_git_progress("remote: Counting objects: 42% (42/100)");
        assert_eq!(parsed, Some((GitProgressStage::Counting, 42)));
    }

    #[test]
    fn test_parse_git_progress_from_carriage_return_line() {
        let parsed = parse_git_progress("Receiving objects:  7% (7/100)\r");
        assert_eq!(parsed, Some((GitProgressStage::Receiving, 7)));
    }

    #[test]
    fn test_git_progress_stage_labels_and_positions() {
        assert_eq!(GitProgressStage::Counting.label(), "Counting objects");
        assert_eq!(GitProgressStage::Compressing.label(), "Compressing objects");
        assert_eq!(GitProgressStage::Receiving.label(), "Receiving objects");
        assert_eq!(GitProgressStage::Resolving.label(), "Resolving deltas");
        assert_eq!(GitProgressStage::Updating.label(), "Writing files");

        assert_eq!(GitProgressStage::Counting.mapped_position(100), 20);
        assert_eq!(GitProgressStage::Compressing.mapped_position(100), 40);
        assert_eq!(GitProgressStage::Receiving.mapped_position(100), 75);
        assert_eq!(GitProgressStage::Resolving.mapped_position(100), 90);
        assert_eq!(GitProgressStage::Updating.mapped_position(100), 95);
        assert_eq!(GitProgressStage::Updating.mapped_position(250), 95);
    }

    #[test]
    fn test_parse_git_progress_all_stage_prefixes() {
        assert_eq!(
            parse_git_progress("Compressing objects: 10% (1/10)"),
            Some((GitProgressStage::Compressing, 10))
        );
        assert_eq!(
            parse_git_progress("Resolving deltas: 55% (11/20)"),
            Some((GitProgressStage::Resolving, 55))
        );
        assert_eq!(
            parse_git_progress("Updating files: 100% (3/3)"),
            Some((GitProgressStage::Updating, 100))
        );
        assert_eq!(parse_git_progress("Counting objects: % (0/0)"), None);
        assert_eq!(parse_git_progress("fatal: repository not found"), None);
    }

    #[test]
    fn test_observe_progress_updates_progress_and_flushes_large_buffer() {
        let progress = ProgressBar::hidden();
        let mut buffer = String::new();

        observe_progress(
            &progress,
            b"remote: Resolving deltas: 80% (8/10)\n",
            &mut buffer,
        );
        assert_eq!(progress.position(), 87);
        assert!(buffer.is_empty());

        observe_progress(&progress, "x".repeat(4097).as_bytes(), &mut buffer);
        assert!(buffer.is_empty());
    }

    #[test]
    fn test_summarize_git_failure_skips_progress_noise() {
        let stderr =
            b"Cloning into 'repo'...\nReceiving objects: 100% (1/1)\nfatal: repository not found\n";
        assert_eq!(summarize_git_failure(stderr), "fatal: repository not found");
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let output = std::process::Command::new("git")
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
