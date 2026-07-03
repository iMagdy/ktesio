use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

#[allow(dead_code)]
#[derive(Debug)]
pub struct KtCommandOutput {
    pub stdout: String,
    pub stderr: String,
}

pub struct TestContext {
    _temp_dir: TempDir,
    pub project_dir: PathBuf,
}

#[allow(dead_code)]
impl TestContext {
    pub fn new() -> Self {
        let temp_dir = TempDir::new().expect("Failed to create temp directory");
        let project_dir = temp_dir.path().join("project");
        std::fs::create_dir_all(&project_dir).expect("Failed to create project directory");

        Self {
            _temp_dir: temp_dir,
            project_dir,
        }
    }

    pub fn skills_dir(&self) -> PathBuf {
        self.project_dir.join(".agents").join("skills")
    }

    pub fn lockfile(&self) -> PathBuf {
        self.project_dir.join("skills.lock")
    }

    pub fn manifest(&self) -> PathBuf {
        self.project_dir.join("skills.json")
    }

    pub fn ensure_skills_dir(&self) {
        std::fs::create_dir_all(self.skills_dir()).expect("Failed to create skills directory");
    }

    pub fn create_fixture_repo(&self, name: &str, with_manifest: bool) -> PathBuf {
        let repo_dir = self.project_dir.join(format!("{name}-fixture"));
        create_local_skill_repo(&repo_dir, name, with_manifest);
        repo_dir
    }
}

pub fn create_local_skill_repo(path: &Path, name: &str, with_manifest: bool) {
    std::fs::create_dir_all(path.join("skills").join(name)).expect("Failed to create skill dir");
    std::fs::write(
        path.join("skills").join(name).join("SKILL.md"),
        format!("# {name}\n\nA local test skill.\n"),
    )
    .expect("Failed to write skill file");
    std::fs::write(
        path.join("README.md"),
        "Repository readme, not a published skill.\n",
    )
    .expect("Failed to write unpublished readme");

    if with_manifest {
        let manifest = serde_json::json!({
            "dependencies": {},
            "publish": [
                {
                    "skill": name,
                    "path": format!("skills/{name}")
                }
            ]
        });
        std::fs::write(
            path.join("skills.json"),
            serde_json::to_string_pretty(&manifest).unwrap(),
        )
        .expect("Failed to write fixture manifest");
    }

    run_git(path, &["init"]);
    run_git(path, &["add", "."]);
    run_git(
        path,
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
}

pub fn run_kt_command(args: &[&str], working_dir: &Path) -> Result<String, String> {
    run_kt_command_output(args, working_dir).map(|output| output.stdout)
}

pub fn run_kt_command_output(args: &[&str], working_dir: &Path) -> Result<KtCommandOutput, String> {
    let output = Command::new(env!("CARGO_BIN_EXE_kt"))
        .args(args)
        .current_dir(working_dir)
        .env("KTESIO_NO_UPDATE_CHECK", "1")
        .output()
        .map_err(|e| format!("Failed to execute kt: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();

    if !output.status.success() {
        return Err(format!("kt failed: {}\n{}", stdout, stderr));
    }

    Ok(KtCommandOutput { stdout, stderr })
}

fn run_git(repo_dir: &Path, args: &[&str]) {
    let output = Command::new("git")
        .args(args)
        .current_dir(repo_dir)
        .output()
        .expect("Failed to run git");

    assert!(
        output.status.success(),
        "git {:?} failed\nstdout:\n{}\nstderr:\n{}",
        args,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
