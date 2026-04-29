use std::collections::BTreeSet;
use std::fs;
use std::path::Path;

#[test]
fn corefile_templates_declare_authorship() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let repo_root = manifest_dir
        .parent()
        .and_then(Path::parent)
        .expect("runtime crate should live under crates/");
    let corefiles_dir = repo_root.join("docs/architecture/corefiles");

    let expected = BTreeSet::from([
        "AGENTS.md",
        "BOOTSTRAP.md",
        "HEARTBEAT.md",
        "IDENTITY.md",
        "MEMORY.md",
        "SOUL.md",
        "TOOLS.md",
        "USER.md",
        "VOICE.md",
    ]);

    let mut seen = BTreeSet::new();
    for entry in fs::read_dir(&corefiles_dir).expect("corefile template directory should exist") {
        let entry = entry.expect("corefile directory entries should be readable");
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }

        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .expect("corefile template names should be valid UTF-8");
        seen.insert(filename.to_string());

        let content = fs::read_to_string(&path).expect("corefile template should be readable");
        let declares_authorship = content.lines().take(3).any(|line| {
            let trimmed = line.trim();
            trimmed.starts_with("<!--") && trimmed.ends_with("-->")
        });
        assert!(
            declares_authorship,
            "{filename} must declare authorship in the first three lines"
        );
    }

    let expected_owned = expected
        .iter()
        .map(|name| (*name).to_string())
        .collect::<BTreeSet<_>>();
    assert_eq!(seen, expected_owned, "corefile template set changed");
}
