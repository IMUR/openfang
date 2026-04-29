//! Regression tests: `agent.toml` is authoritative for declarative manifest on boot;
//! SurrealDB is refreshed; sync persist paths must await memory futures.
//!
//! Run: `cargo test -p openfang-kernel --test agent_manifest_authority_test`

use openfang_kernel::OpenFangKernel;
use openfang_types::agent::{AgentManifest, ToolProfile};
use openfang_types::config::{DefaultModelConfig, KernelConfig};
use std::path::Path;

fn test_kernel_config(home: &Path) -> KernelConfig {
    KernelConfig {
        home_dir: home.to_path_buf(),
        data_dir: home.join("data"),
        default_model: DefaultModelConfig {
            provider: "groq".to_string(),
            model: "llama-3.3-70b-versatile".to_string(),
            api_key_env: "GROQ_API_KEY".to_string(),
            base_url: None,
        },
        spawn_default_assistant_on_empty_registry: false,
        ..KernelConfig::default()
    }
}

const AGENT_NAME: &str = "ManifestAuthAgent";

fn disk_manifest_toml(temperature: f32, profile: &str) -> String {
    format!(
        r#"
name = "{AGENT_NAME}"
version = "0.1.0"
description = "manifest authority test"
author = "test"
module = "builtin:chat"
profile = "{profile}"
skills = ["rtr-openfang"]
mcp_servers = ["emcp-global"]

[model]
provider = "groq"
model = "llama-3.3-70b-versatile"
system_prompt = "Test prompt."
temperature = {temperature}
max_tokens = 1024

[capabilities]
network = ["web_fetch"]
tools = ["file_read"]
memory_read = ["*"]
memory_write = ["self.*"]
"#
    )
}

#[tokio::test(flavor = "multi_thread")]
async fn disk_manifest_wins_over_stale_db_after_restart() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();
    let config = test_kernel_config(home);

    // --- Boot 1: spawn agent; DB records temperature = 0.2 ---
    let kernel = OpenFangKernel::boot_with_config(config.clone())
        .await
        .expect("boot");

    let mut manifest: AgentManifest =
        toml::from_str(&disk_manifest_toml(0.2, "minimal")).expect("parse manifest");
    manifest.mcp_servers = vec![]; // avoid validation against live MCP in test

    let _id = kernel.spawn_agent(manifest).expect("spawn first");
    kernel.shutdown();
    std::mem::drop(kernel);

    // --- Overwrite on-disk template only (simulates user editing agent.toml) ---
    let toml_path = home.join("agents").join(AGENT_NAME).join("agent.toml");
    std::fs::create_dir_all(toml_path.parent().unwrap()).unwrap();
    std::fs::write(&toml_path, disk_manifest_toml(0.88, "full")).expect("write toml");

    // --- Boot 2: must load full disk manifest (temperature + profile), not DB-only diff ---
    let kernel2 = OpenFangKernel::boot_with_config(config)
        .await
        .expect("boot 2");

    let entry = kernel2
        .registry
        .find_by_name(AGENT_NAME)
        .expect("agent in registry");
    assert!(
        (entry.manifest.model.temperature - 0.88).abs() < f32::EPSILON,
        "expected disk temperature 0.88, got {}",
        entry.manifest.model.temperature
    );
    assert_eq!(entry.manifest.profile, Some(ToolProfile::Full));
    assert_eq!(entry.manifest.skills, vec!["rtr-openfang".to_string()]);
    assert!(!entry.manifest.mcp_servers.is_empty());

    kernel2.shutdown();
}

#[tokio::test(flavor = "multi_thread")]
async fn boots_when_agent_toml_missing_uses_db_manifest() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let home = tmp.path();
    let config = test_kernel_config(home);

    let kernel = OpenFangKernel::boot_with_config(config.clone())
        .await
        .expect("boot");
    let mut manifest: AgentManifest =
        toml::from_str(&disk_manifest_toml(0.5, "minimal")).expect("parse");
    manifest.mcp_servers = vec![];
    let _id = kernel.spawn_agent(manifest).expect("spawn");
    kernel.shutdown();
    std::mem::drop(kernel);

    // Remove template on disk; DB still has the agent
    let toml_path = home.join("agents").join(AGENT_NAME).join("agent.toml");
    let _ = std::fs::remove_file(&toml_path);

    let kernel2 = OpenFangKernel::boot_with_config(test_kernel_config(home))
        .await
        .expect("boot 2");
    let entry = kernel2
        .registry
        .find_by_name(AGENT_NAME)
        .expect("restored from DB");
    assert!(
        (entry.manifest.model.temperature - 0.5).abs() < f32::EPSILON,
        "DB fallback should preserve manifest"
    );
    kernel2.shutdown();
}
