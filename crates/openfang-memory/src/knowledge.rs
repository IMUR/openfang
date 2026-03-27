//! Knowledge graph backed by SurrealDB.
//!
//! Entities are stored as documents in the `entities` table.
//! Relations use SurrealDB's native RELATE graph edges.

use chrono::Utc;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{
    Entity, EntityType, GraphMatch, GraphPattern, Relation, RelationType,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::db::SurrealDb;

/// Knowledge graph store backed by SurrealDB.
#[derive(Clone)]
pub struct KnowledgeStore {
    db: SurrealDb,
}

/// Entity record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct EntityRecord {
    entity_type: EntityType,
    name: String,
    properties: std::collections::HashMap<String, serde_json::Value>,
    created_at: String,
    updated_at: String,
}

/// Relation record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct RelationRecord {
    relation_type: RelationType,
    properties: std::collections::HashMap<String, serde_json::Value>,
    confidence: f32,
    created_at: String,
}

/// Raw graph query result row.
#[derive(Debug, Deserialize)]
struct GraphRow {
    source: EntityRecord,
    relation: RelationRecord,
    target: EntityRecord,
    source_id: String,
    target_id: String,
    relation_source: String,
    relation_target: String,
}

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

impl KnowledgeStore {
    /// Create a new knowledge store wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Add an entity to the knowledge graph.
    pub async fn add_entity(&self, entity: Entity) -> OpenFangResult<String> {
        let id = if entity.id.is_empty() {
            Uuid::new_v4().to_string()
        } else {
            entity.id.clone()
        };
        let now = Utc::now().to_rfc3339();

        let _: Option<EntityRecord> = self
            .db
            .upsert(("entities", id.as_str()))
            .content(EntityRecord {
                entity_type: entity.entity_type,
                name: entity.name,
                properties: entity.properties,
                created_at: now.clone(),
                updated_at: now,
            })
            .await
            .map_err(surreal_err)?;

        Ok(id)
    }

    /// Add a relation between two entities.
    pub async fn add_relation(&self, relation: Relation) -> OpenFangResult<String> {
        let id = Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();

        // Use SurrealQL RELATE to create a graph edge
        self.db
            .query(
                "RELATE (type::thing('entities', $source))->(type::thing('relations', $id))->(type::thing('entities', $target))
                 SET relation_type = $rel_type,
                     properties = $props,
                     confidence = $conf,
                     created_at = $now"
            )
            .bind(("source", relation.source.clone()))
            .bind(("target", relation.target.clone()))
            .bind(("id", id.clone()))
            .bind(("rel_type", serde_json::to_value(&relation.relation).map_err(|e| OpenFangError::Serialization(e.to_string()))?))
            .bind(("props", serde_json::to_value(&relation.properties).map_err(|e| OpenFangError::Serialization(e.to_string()))?))
            .bind(("conf", relation.confidence as f64))
            .bind(("now", now))
            .await
            .map_err(surreal_err)?;

        Ok(id)
    }

    /// Query the knowledge graph with a pattern.
    pub async fn query_graph(&self, pattern: GraphPattern) -> OpenFangResult<Vec<GraphMatch>> {
        // Build a dynamic SurrealQL query with optional filters
        let mut conditions = Vec::new();
        let mut bindings: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(ref source) = pattern.source {
            conditions.push("(source_id = $source_filter OR s.name = $source_filter)");
            bindings.push(("source_filter".into(), serde_json::json!(source)));
        }
        if let Some(ref relation) = pattern.relation {
            conditions.push("r.relation_type = $rel_filter");
            bindings.push((
                "rel_filter".into(),
                serde_json::to_value(relation)
                    .map_err(|e| OpenFangError::Serialization(e.to_string()))?,
            ));
        }
        if let Some(ref target) = pattern.target {
            conditions.push("(target_id = $target_filter OR t.name = $target_filter)");
            bindings.push(("target_filter".into(), serde_json::json!(target)));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        let sql = format!(
            "SELECT
                s.* AS source,
                r.* AS relation,
                t.* AS target,
                meta::id(s.id) AS source_id,
                meta::id(t.id) AS target_id,
                r.in AS relation_source,
                r.out AS relation_target
             FROM relations r
             LET s = r.in,
                 t = r.out
             {where_clause}
             LIMIT 100"
        );

        let mut query = self.db.query(&sql);
        for (key, val) in bindings {
            query = query.bind((key, val));
        }

        let mut result = query.await.map_err(surreal_err)?;
        let rows: Vec<GraphRow> = result.take(0).unwrap_or_default();

        let matches = rows
            .into_iter()
            .map(|row| {
                let created = |s: &str| {
                    chrono::DateTime::parse_from_rfc3339(s)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now())
                };

                GraphMatch {
                    source: Entity {
                        id: row.source_id,
                        entity_type: row.source.entity_type,
                        name: row.source.name,
                        properties: row.source.properties,
                        created_at: created(&row.source.created_at),
                        updated_at: created(&row.source.updated_at),
                    },
                    relation: Relation {
                        source: row.relation_source,
                        relation: row.relation.relation_type,
                        target: row.relation_target,
                        properties: row.relation.properties,
                        confidence: row.relation.confidence,
                        created_at: created(&row.relation.created_at),
                    },
                    target: Entity {
                        id: row.target_id,
                        entity_type: row.target.entity_type,
                        name: row.target.name,
                        properties: row.target.properties,
                        created_at: created(&row.target.created_at),
                        updated_at: created(&row.target.updated_at),
                    },
                }
            })
            .collect();

        Ok(matches)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use std::collections::HashMap;

    async fn setup() -> KnowledgeStore {
        let db = db::init_mem().await.unwrap();
        KnowledgeStore::new(db)
    }

    #[tokio::test]
    async fn test_add_and_query_entity() {
        let store = setup().await;
        let id = store
            .add_entity(Entity {
                id: String::new(),
                entity_type: EntityType::Person,
                name: "Alice".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        assert!(!id.is_empty());
    }

    #[tokio::test]
    async fn test_add_relation() {
        let store = setup().await;
        let alice_id = store
            .add_entity(Entity {
                id: "alice".to_string(),
                entity_type: EntityType::Person,
                name: "Alice".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let company_id = store
            .add_entity(Entity {
                id: "acme".to_string(),
                entity_type: EntityType::Organization,
                name: "Acme Corp".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let rel_id = store
            .add_relation(Relation {
                source: alice_id,
                relation: RelationType::WorksAt,
                target: company_id,
                properties: HashMap::new(),
                confidence: 0.95,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        assert!(!rel_id.is_empty());
    }
}
