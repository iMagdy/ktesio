mod helpers;

use helpers::{run_kt_command, run_kt_command_output, TestContext};

#[test]
fn test_install_single_skill_creates_directory() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    // Create a skills.json pointing to the local fixture
    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Run kt install
    let result = run_kt_command(&["install"], &ctx.project_dir);
    assert!(result.is_ok(), "kt install failed: {:?}", result.err());

    // Verify skill directory was created
    let skill_dir = ctx.skills_dir().join("awesome-copilot");
    assert!(skill_dir.exists(), "Skill directory should exist");
}

#[test]
fn test_install_single_skill_updates_lockfile() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    // Create skills.json
    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Run kt install
    let result = run_kt_command(&["install"], &ctx.project_dir);
    assert!(result.is_ok(), "kt install failed: {:?}", result.err());

    // Verify lockfile was created and contains the skill
    assert!(ctx.lockfile().exists(), "Lockfile should exist");
    let lockfile_content = std::fs::read_to_string(ctx.lockfile()).unwrap();
    assert!(
        lockfile_content.contains("awesome-copilot"),
        "Lockfile should contain skill name"
    );
}

#[test]
fn test_publish_add_keeps_skills_json() {
    let ctx = TestContext::new();
    std::fs::create_dir_all(ctx.project_dir.join("skills/local")).unwrap();
    std::fs::write(ctx.project_dir.join("skills/local/SKILL.md"), "# Local").unwrap();

    let result = run_kt_command(
        &["publish", "add", "local", "skills/local"],
        &ctx.project_dir,
    );
    assert!(result.is_ok(), "kt publish add failed: {:?}", result.err());

    // Verify skills.json exists and contains the skill
    assert!(ctx.manifest().exists(), "skills.json should exist");
    let manifest_content = std::fs::read_to_string(ctx.manifest()).unwrap();
    assert!(
        manifest_content.contains("local"),
        "Manifest should contain skill name"
    );
}

#[test]
fn test_install_completes_within_30_seconds() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    // Create skills.json
    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let start = std::time::Instant::now();
    let result = run_kt_command(&["install"], &ctx.project_dir);
    let duration = start.elapsed();

    assert!(result.is_ok(), "kt install failed: {:?}", result.err());
    assert!(
        duration.as_secs() < 30,
        "Install should complete within 30 seconds, took {:?}",
        duration
    );
}

#[test]
fn test_install_clones_correct_content() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    // Create skills.json
    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Run kt install
    let result = run_kt_command(&["install"], &ctx.project_dir);
    assert!(result.is_ok(), "kt install failed: {:?}", result.err());

    // Verify skill directory contains files
    let skill_dir = ctx.skills_dir().join("awesome-copilot");
    let entries: Vec<_> = std::fs::read_dir(&skill_dir).unwrap().collect();
    assert!(!entries.is_empty(), "Skill directory should not be empty");
    assert!(
        skill_dir.join("SKILL.md").exists(),
        "Published skill content should be installed"
    );
    assert!(
        !skill_dir.join("README.md").exists(),
        "Unpublished repo files should not be installed"
    );
    assert!(
        !skill_dir.join(".git").exists(),
        "Git metadata should not be installed"
    );
}

#[test]
fn test_install_hides_raw_git_clone_output() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    let output =
        run_kt_command_output(&["install"], &ctx.project_dir).expect("kt install should succeed");

    assert!(
        !output.stderr.contains("Cloning into"),
        "raw git clone output should be hidden: {}",
        output.stderr
    );
    assert!(
        !output.stderr.contains("Receiving objects"),
        "raw git progress output should be hidden: {}",
        output.stderr
    );
}

#[test]
fn test_install_does_not_modify_existing_skills() {
    let ctx = TestContext::new();
    ctx.ensure_skills_dir();

    // Create an existing skill
    let existing_skill_dir = ctx.skills_dir().join("existing-skill");
    std::fs::create_dir_all(&existing_skill_dir).unwrap();
    std::fs::write(existing_skill_dir.join("file.txt"), "existing content").unwrap();

    let fixture_dir = ctx.create_fixture_repo("awesome-copilot", true);

    // Create skills.json with new skill
    let manifest = serde_json::json!({
        "dependencies": {
            "awesome-copilot": {
                "repo": fixture_dir.to_str().unwrap()
            }
        },
        "publish": []
    });
    std::fs::write(
        ctx.manifest(),
        serde_json::to_string_pretty(&manifest).unwrap(),
    )
    .unwrap();

    // Run kt install
    let result = run_kt_command(&["install"], &ctx.project_dir);
    assert!(result.is_ok(), "kt install failed: {:?}", result.err());

    // Verify existing skill was not modified
    assert!(
        existing_skill_dir.exists(),
        "Existing skill should still exist"
    );
    let content = std::fs::read_to_string(existing_skill_dir.join("file.txt")).unwrap();
    assert_eq!(
        content, "existing content",
        "Existing skill content should not be modified"
    );
}
