import re

files = [
    'crates/openfang-api/src/routes.rs',
]

async_methods = [
    'get_session', 'save_session', 'list_kv', 'structured_get',
    'structured_set', 'structured_delete', 'save_agent',
    'list_sessions', 'delete_session', 'set_session_label',
    'find_session_by_label', 'budget_status', 'compact_agent_session', 'send_message', 'session_usage_cost',
    'load_all_agents', 'run_agent_loop_streaming', 'reset_session', 'clear_agent_history', 'list_agent_sessions',
    'create_agent_session', 'switch_agent_session', 'context_report'
]

def fix_file(path):
    with open(path, 'r') as f:
        src = f.read()

    for method in async_methods:
        src = re.sub(rf'(\.(?:memory|kernel|metering|scheduler|registry)\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(self\.kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(state\.kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)

    if 'routes.rs' in path:
        src = src.replace('pub fn inject_attachments_into_session(', 'pub async fn inject_attachments_into_session(')
        src = src.replace('        inject_attachments_into_session(&state.kernel, agent_id, blocks);', '        inject_attachments_into_session(&state.kernel, agent_id, blocks).await;')
        src = src.replace('match state.kernel.spawn_agent(manifest).await {', 'match state.kernel.spawn_agent(manifest) {')
        src = src.replace('state.kernel.stop_agent_run(agent_id).await.unwrap_or(false)', 'state.kernel.stop_agent_run(agent_id).unwrap_or(false)')
        src = src.replace('    ) {\n        Ok(pair) => pair,', '    ).await {\n        Ok(pair) => pair,')
        src = src.replace('.set_agent_model(agent_id, new_model, None).await', '.set_agent_model(agent_id, new_model, None)')
        
        # fix structured_get issues when .ok() was chained incorrectly
        src = re.sub(r'\.structured_get\((.*?)\)\n\s*\.await\.ok\(\)', r'.structured_get(\1).await.ok()', src)
        src = re.sub(r'\.structured_get\((.*?)\)\.ok\(\)', r'.structured_get(\1).await.ok()', src)
        src = re.sub(r'memory\.structured_get\((.*?)\)\.is_ok\(\)', r'memory.structured_get(\1).await.is_ok()', src)

        # eliminate sqlite legacy manually but specifically!
        src = src.replace('let usage_store = openfang_memory::usage::UsageStore::new(state.kernel.memory.usage_conn());', '')

    with open(path, 'w') as f:
        f.write(src)

for fn in files:
    try:
        fix_file(fn)
    except Exception as e:
        print(e)
