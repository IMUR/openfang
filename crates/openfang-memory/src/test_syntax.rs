use surrealdb::engine::local::Mem;
use surrealdb::Surreal;

#[tokio::main]
async fn main() {
    let db = Surreal::new::<Mem>(()).await.unwrap();
    db.use_ns("test").use_db("test").await.unwrap();
    
    // Test the DISTINCT syntax
    let res = db.query("SELECT DISTINCT agent_id FROM memories WHERE deleted = false").await;
    println!("DISTINCT: {:?}", res);

    // Test GROUP BY syntax
    let res2 = db.query("SELECT agent_id FROM memories WHERE deleted = false GROUP BY agent_id").await;
    println!("GROUP BY: {:?}", res2);
}
