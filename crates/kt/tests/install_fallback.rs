mod helpers;

use helpers::{run_kt_command, run_kt_command_output, TestContext};

#[test]
fn test_install_fallback_displays_warning() {
    let ctx = TestContext::new();

    let fixture_dir = ctx.project_dir.join("fixture");
    std::fs::create_dir_all(fixture_dir.join("skills/agent-skill")).unwrap();
    std::fs::write(
        fixture_dir.join("skills/agent-skill/SKILL.md"),
        "# Agent Skill",
    )
    .unwrap();

    let output =
        run_kt_command_output(&["install"], &fixture_dir).expect("fallback install should succeed");

    assert!(output.stderr.contains("No skills.json found"));
    assert!(fixture_dir
        .join(".agents/skills/agent-skill/SKILL.md")
        .exists());
    let lockfile = std::fs::read_to_string(fixture_dir.join("skills.lock")).unwrap();
    assert!(lockfile.contains("agent-skill"));
}

#[test]
fn test_install_fallback_discovers_skills_directory() {
    let ctx = TestContext::new();

    // Create a local directory structure that mimics a repo without skills.json
    let fixture_dir = ctx.project_dir.join("fixture");
    std::fs::create_dir_all(fixture_dir.join("skills")).unwrap();
    std::fs::write(fixture_dir.join("skills/test-skill.md"), "# Test Skill").unwrap();

    let result = run_kt_command(&["install"], &fixture_dir);

    assert!(
        result.is_ok(),
        "fallback install should succeed: {:?}",
        result.err()
    );
    assert!(fixture_dir
        .join(".agents/skills/test-skill/test-skill.md")
        .exists());
    assert!(fixture_dir.join("skills.lock").exists());
}

#[test]
fn test_install_fallback_discovers_agents_skills_without_self_copy() {
    let ctx = TestContext::new();

    let fixture_dir = ctx.project_dir.join("fixture");
    std::fs::create_dir_all(fixture_dir.join(".agents/skills/local-skill")).unwrap();
    std::fs::write(
        fixture_dir.join(".agents/skills/local-skill/SKILL.md"),
        "# Local Skill",
    )
    .unwrap();

    let result = run_kt_command(&["install"], &fixture_dir);

    assert!(
        result.is_ok(),
        "fallback install should succeed: {:?}",
        result.err()
    );
    assert!(fixture_dir
        .join(".agents/skills/local-skill/SKILL.md")
        .exists());
    let lockfile = std::fs::read_to_string(fixture_dir.join("skills.lock")).unwrap();
    assert!(lockfile.contains("local-skill"));
}

#[test]
fn test_install_fallback_error_missing_skills_dir() {
    let ctx = TestContext::new();

    // Create a directory without skills/, SKILLS/, or .agents/skills/
    let fixture_dir = ctx.project_dir.join("fixture");
    std::fs::create_dir_all(&fixture_dir).unwrap();

    // Run kt install - this should fail because no skills directory exists
    let result = run_kt_command(&["install"], &fixture_dir);

    // Should fail because no skills directory found
    assert!(
        result.is_err(),
        "Should fail when no skills directory exists"
    );
}

#[test]
fn test_install_fallback_error_empty_skills_dir() {
    let ctx = TestContext::new();

    // Create a directory with empty skills/
    let fixture_dir = ctx.project_dir.join("fixture");
    std::fs::create_dir_all(fixture_dir.join("skills")).unwrap();

    // Run kt install - this should fail because skills directory is empty
    let result = run_kt_command(&["install"], &fixture_dir);

    // Should fail because skills directory is empty
    assert!(
        result.is_err(),
        "Should fail when skills directory is empty"
    );
}
