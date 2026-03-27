import re
import os

files = [
    'crates/openfang-api/src/routes.rs',
    'crates/openfang-api/src/channel_bridge.rs',
    'crates/openfang-api/src/ws.rs',
    'crates/openfang-cli/src/tui/mod.rs'
]

async_methods = [
    "get_session", "save_session", "list_kv", "structured_get",
    "structured_set", "structured_delete", "save_agent",
    "list_sessions", "delete_session", "set_session_label",
    "find_session_by_label", "budget_status", "compact_agent_session", "stop_agent_run", "send_message", "session_usage_cost",
    "set_agent_model", "load_all_agents", "run_agent_loop_streaming", "spawn_agent"
]

def fix_file(path):
    with open(path, 'r') as f:
        src = f.read()
    
    # Simple regex to catch method calls and add .await
    for method in async_methods:
        # Match `object.method(args)` where it does NOT have `.await`
        # Note: simplistic matching that handles common cases
        src = re.sub(rf'(\.memory\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(\.kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(self\.kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(state\.kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
        src = re.sub(rf'(kernel\.{method}\s*\([^)]*\))(?!\.await)', r'\1.await', src)
    
    # In channel_bridge.rs: 
    src = re.sub(r'status\.(hourly_spend|hourly_limit|hourly_pct|daily_spend|daily_limit|daily_pct|monthly_spend|monthly_limit|monthly_pct|alert_threshold)', r'status.await.\1', src)
    
    # In routes.rs: stub out usage_conn because it's dead
    # Let's cleanly replace the entire agent_budget_status / agent_usage endpoints
    if "routes.rs" in path:
        # We replace the usage_conn() logic with empty logic or .usage() logic
        pass

    with open(path, 'w') as f:
        f.write(src)

for fn in files:
    try:
        fix_file(fn)
    except Exception:
        pass
