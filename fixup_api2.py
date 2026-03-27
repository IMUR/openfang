import sys

def modify_routes():
    path = "crates/openfang-api/src/routes.rs"
    with open(path, "r") as f:
        src = f.read()
    
    # 1. Replace all simple async calls with .await.
    # Note: we also have to fix unwraps
    methods = ["get_session", "save_session", "list_kv", "structured_get", 
               "structured_set", "structured_delete", "save_agent", "list_sessions", 
               "delete_session", "set_session_label", "find_session_by_label"]
    
    for m in methods:
        src = src.replace(f"kernel.memory.{m}(", f"kernel.memory.{m}(")
    # Wait, simple replace won't work well due to line breaks.
    
    with open(path, "w") as f:
        f.write(src)
