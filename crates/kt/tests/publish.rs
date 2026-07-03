mod helpers;

use helpers::{run_kt_command, TestContext};

#[test]
fn test_export_command_is_removed() {
    let ctx = TestContext::new();

    let result = run_kt_command(&["export"], &ctx.project_dir);

    assert!(result.is_err(), "kt export should no longer exist");
}

#[test]
fn test_publish_add_creates_publish_entry() {
    let ctx = TestContext::new();
    std::fs::create_dir_all(ctx.project_dir.join("skills/local")).unwrap();
    std::fs::write(ctx.project_dir.join("skills/local/SKILL.md"), "# Local").unwrap();

    let result = run_kt_command(
        &["publish", "add", "local", "skills/local"],
        &ctx.project_dir,
    );

    assert!(result.is_ok(), "kt publish add failed: {:?}", result.err());
    let manifest: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(ctx.manifest()).unwrap()).unwrap();
    assert!(manifest.get("dependencies").is_some());
    assert_eq!(manifest["publish"][0]["skill"], "local");
    assert_eq!(manifest["publish"][0]["path"], "skills/local");
}

#[test]
fn test_publish_add_preserves_manifest_json() {
    let ctx = TestContext::new();
    std::fs::create_dir_all(ctx.project_dir.join("skills/local")).unwrap();
    std::fs::write(ctx.project_dir.join("skills/local/SKILL.md"), "# Local").unwrap();
    std::fs::write(
        ctx.manifest(),
        r#"{"dependencies": {"docs": {"repo": "url"}}, "publish": []}"#,
    )
    .unwrap();

    let result = run_kt_command(
        &["publish", "add", "local", "skills/local"],
        &ctx.project_dir,
    );

    assert!(result.is_ok(), "kt publish add failed: {:?}", result.err());
    let manifest_content = std::fs::read_to_string(ctx.manifest()).unwrap();
    let manifest: serde_json::Value = serde_json::from_str(&manifest_content).unwrap();
    assert_eq!(manifest["dependencies"]["docs"]["repo"], "url");
    assert!(manifest_content.contains("  "));
}
