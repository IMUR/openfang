use surrealdb::engine::local::SurrealKv;
use surrealdb::Surreal;
use serde_json::Value;

#[tokio::test]
async fn audit_live_database() -> Result<(), Box<dyn std::error::Error>> {
    // Use the ACTUAL data path from config, not in-memory
    let db_path = std::env::var("OPENFANG_DB_PATH")
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap();
            format!("{home}/.openfang/data/openfang.db")
        });
    
    eprintln!("=== AUDIT: Opening {db_path} ===");
    
    let db = Surreal::new::<SurrealKv>(&db_path).await?;
    db.use_ns("openfang").use_db("agents").await?;

    // 1. What tables exist?
    let tables: Option<Value> = db.query("INFO FOR DB").await?.take(0)?;
    eprintln!("=== TABLES ===\n{:#?}", tables);

    // 2. For each known table, how many records?
    let tables_to_check = [
        "sessions", "canonical_sessions", "memories", "kv", 
        "agents", "usage", "entities", "relations",
        "paired_devices", "task_queue"
    ];
    for table in tables_to_check {
        let count: Option<Value> = db
            .query(format!("SELECT count() FROM {table} GROUP ALL"))
            .await?
            .take(0)?;
        eprintln!("  {table}: {:?}", count);
    }

    // 3. What indexes exist on memories?
    let mem_info: Option<Value> = db
        .query("INFO FOR TABLE memories")
        .await?
        .take(0)?;
    eprintln!("=== MEMORIES TABLE INFO ===\n{:#?}", mem_info);

    // 4. What embedding dimensions are actually stored?
    let dims: Vec<Value> = db
        .query("SELECT array::len(embedding) AS dim FROM memories WHERE type::of(embedding) = 'array' LIMIT 5")
        .await?
        .take(0)?;
    eprintln!("=== EMBEDDING DIMENSIONS (sample) ===\n{:#?}", dims);

    // 5. Are there any memories at all? Sample one.
    let sample: Vec<Value> = db
        .query("SELECT meta::id(id) AS id, agent_id, string::len(content) AS content_len, embedding IS NOT NONE AS has_embedding, confidence, type::of(created_at) AS created_at_type FROM memories LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== MEMORY SAMPLES ===\n{:#?}", sample);

    // 6. Check entities and relations (knowledge graph)
    let ent_sample: Vec<Value> = db
        .query("SELECT meta::id(id) AS id, entity_type, name FROM entities LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== ENTITY SAMPLES ===\n{:#?}", ent_sample);

    let rel_sample: Vec<Value> = db
        .query("SELECT meta::id(id) AS id, in, out, relation_type, confidence FROM relations LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== RELATION SAMPLES ===\n{:#?}", rel_sample);

    // 7. Check usage records
    let usage_sample: Vec<Value> = db
        .query("SELECT id, agent_id, model, input_tokens, output_tokens, cost_usd FROM usage ORDER BY id DESC LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== USAGE SAMPLES ===\n{:#?}", usage_sample);

    // 8. Check sessions
    let sess_count: Vec<Value> = db
        .query("SELECT agent_id, array::len(messages) AS msg_count FROM sessions LIMIT 5")
        .await?
        .take(0)?;
    eprintln!("=== SESSION SAMPLES ===\n{:#?}", sess_count);

    // 9. Agent entries (check for double-serialization)
    let agent_sample: Vec<Value> = db
        .query("SELECT name, type::of(data) AS data_type, string::len(<string>data) AS data_len FROM agents LIMIT 3")
        .await?
        .take(0)?;
    eprintln!("=== AGENT SAMPLES ===\n{:#?}", agent_sample);

    // 10. Check for any DEFINE INDEX / ANALYZER already present
    for table in ["memories", "sessions", "entities", "relations", "usage"] {
        let info: Option<Value> = db
            .query(format!("INFO FOR TABLE {table}"))
            .await?
            .take(0)?;
        eprintln!("=== INFO FOR {table} ===\n{:#?}", info);
    }

    eprintln!("=== AUDIT COMPLETE ===");
    Ok(())
}
