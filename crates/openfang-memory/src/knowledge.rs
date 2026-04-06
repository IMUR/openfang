//! Knowledge graph backed by SurrealDB.
//!
//! Entities are stored as documents in the `entities` table.
//! Relations use SurrealDB's native RELATE graph edges.
//!
//! ## Traversal
//!
//! - `query_graph` — flat edge scan with optional source/relation/target filters.
//!   Suitable for pattern matching ("find all WorksAt relations").
//!
//! - `traverse_from` — multi-hop graph traversal using SurrealDB's native `->` operator.
//!   More efficient for fan-out from a known source entity (up to 3 hops).

use chrono::Utc;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{
    Entity, EntityType, GraphMatch, GraphPattern, Relation, RelationType,
};

/// A node reached during graph traversal, with the hop depth it was found at.
#[derive(Debug, Clone)]
pub struct TraversalNode {
    pub entity: Entity,
    /// 1 = direct neighbour, 2 = two hops away, etc.
    pub depth: usize,
    /// The ID of the entity that linked to this node.
    pub via_entity_id: String,
}
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

// ---------------------------------------------------------------------------
// Traversal helper types and functions
// ---------------------------------------------------------------------------

/// Raw entity record as returned by a graph traversal hop.
#[derive(Debug, Deserialize)]
struct RawEntity {
    #[serde(rename = "id")]
    raw_id: Option<surrealdb::RecordId>,
    entity_type: Option<EntityType>,
    name: Option<String>,
    #[serde(default)]
    properties: std::collections::HashMap<String, serde_json::Value>,
    created_at: Option<String>,
    updated_at: Option<String>,
}

/// Result row from the multi-hop traversal query.
#[derive(Debug, Deserialize)]
struct TraversalResult {
    #[allow(dead_code)]
    eid: Option<String>,
    #[serde(default)]
    hop1: Vec<RawEntity>,
    #[serde(default)]
    hop2: Vec<RawEntity>,
    #[serde(default)]
    hop3: Vec<RawEntity>,
}

/// Convert a batch of raw entities from one hop into `TraversalNode`s, deduplicating.
fn collect_hop(
    raw_ents: Vec<RawEntity>,
    hop_depth: usize,
    via: &str,
    seen: &mut std::collections::HashSet<String>,
    nodes: &mut Vec<TraversalNode>,
) {
    for re in raw_ents {
        let eid = re
            .raw_id
            .as_ref()
            .map(|rid| format!("{}", rid.key()))
            .unwrap_or_default();
        if eid.is_empty() || !seen.insert(eid.clone()) {
            continue;
        }
        let now = Utc::now();
        let parse_ts = |s: Option<&str>| -> chrono::DateTime<Utc> {
            s.and_then(|ts| {
                chrono::DateTime::parse_from_rfc3339(ts)
                    .ok()
                    .map(|dt| dt.with_timezone(&Utc))
            })
            .unwrap_or(now)
        };
        let entity = Entity {
            id: eid.clone(),
            entity_type: re.entity_type.unwrap_or(EntityType::Custom("unknown".to_string())),
            name: re.name.unwrap_or_default(),
            properties: re.properties,
            created_at: parse_ts(re.created_at.as_deref()),
            updated_at: parse_ts(re.updated_at.as_deref()),
        };
        nodes.push(TraversalNode {
            entity,
            depth: hop_depth,
            via_entity_id: via.to_string(),
        });
    }
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
            // `in` is the source record link on the edge; compare by id string or name
            conditions.push("(meta::id(in) = $source_filter OR in.name = $source_filter)");
            bindings.push(("source_filter".into(), serde_json::json!(source)));
        }
        if let Some(ref relation) = pattern.relation {
            conditions.push("relation_type = $rel_filter");
            bindings.push((
                "rel_filter".into(),
                serde_json::to_value(relation)
                    .map_err(|e| OpenFangError::Serialization(e.to_string()))?,
            ));
        }
        if let Some(ref target) = pattern.target {
            // `out` is the target record link on the edge
            conditions.push("(meta::id(out) = $target_filter OR out.name = $target_filter)");
            bindings.push(("target_filter".into(), serde_json::json!(target)));
        }

        let where_clause = if conditions.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", conditions.join(" AND "))
        };

        // SurrealDB edge records have `in` (source RecordId) and `out` (target RecordId).
        // Using `in.*` / `out.*` auto-fetches the linked entity fields in a single query.
        // `relations` must be backtick-quoted because it is a reserved word in SurrealDB.
        // The LET-alias pattern used previously is not valid SurrealQL syntax.
        let sql = format!(
            "SELECT
                in.* AS source,
                out.* AS target,
                {{ relation_type: relation_type, properties: properties,
                   confidence: confidence, created_at: created_at }} AS relation,
                meta::id(in) AS source_id,
                meta::id(out) AS target_id,
                meta::id(in) AS relation_source,
                meta::id(out) AS relation_target
             FROM `relations`
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

    /// Multi-hop graph traversal from a source entity using SurrealDB's `->` operator.
    ///
    /// Returns all entities reachable within `max_depth` hops from `source_id`,
    /// deduplicating across hops. Depth 1 = direct neighbours, depth 2 = two hops, etc.
    ///
    /// SurrealDB's graph traversal is significantly more efficient than the flat
    /// `query_graph` method for fan-out queries — it avoids a full table scan of
    /// the `relations` edge table.
    pub async fn traverse_from(
        &self,
        source_id: &str,
        max_depth: usize,
    ) -> OpenFangResult<Vec<TraversalNode>> {
        let depth = max_depth.clamp(1, 3);

        // Build the traversal projection for each requested depth.
        // Each hop: ->relations->entities with full entity fields.
        // We use a single query that projects all hops; SurrealDB resolves them
        // in one round-trip via index-backed edge lookups.
        let sql = match depth {
            1 => {
                "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1
                 FROM type::thing('entities', $src)"
                .to_string()
            }
            2 => {
                "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1,
                        (->relations->entities->relations->entities.*) AS hop2
                 FROM type::thing('entities', $src)"
                .to_string()
            }
            _ => {
                "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1,
                        (->relations->entities->relations->entities.*) AS hop2,
                        (->relations->entities->relations->entities->relations->entities.*) AS hop3
                 FROM type::thing('entities', $src)"
                .to_string()
            }
        };

        let mut result = self
            .db
            .query(&sql)
            .bind(("src", source_id.to_string()))
            .await
            .map_err(surreal_err)?;

        let rows: Vec<TraversalResult> = result.take(0).unwrap_or_default();

        let mut nodes: Vec<TraversalNode> = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(source_id.to_string());
        let root_id = source_id.to_string();

        for row in rows {
            collect_hop(row.hop1, 1, &root_id, &mut seen, &mut nodes);
            if depth >= 2 {
                collect_hop(row.hop2, 2, &root_id, &mut seen, &mut nodes);
            }
            if depth >= 3 {
                collect_hop(row.hop3, 3, &root_id, &mut seen, &mut nodes);
            }
        }

        Ok(nodes)
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
    async fn test_traverse_from() {
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

        let acme_id = store
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

        store
            .add_relation(Relation {
                source: alice_id.clone(),
                relation: RelationType::WorksAt,
                target: acme_id.clone(),
                properties: HashMap::new(),
                confidence: 0.9,
                created_at: Utc::now(),
            })
            .await
            .unwrap();

        // Depth-1 traversal from alice should reach Acme Corp
        let nodes = store.traverse_from(&alice_id, 1).await.unwrap();
        assert!(!nodes.is_empty(), "Expected at least one node in traversal");
        let names: Vec<&str> = nodes.iter().map(|n| n.entity.name.as_str()).collect();
        assert!(
            names.contains(&"Acme Corp"),
            "Expected Acme Corp in traversal: {:?}",
            names
        );
        assert_eq!(nodes[0].depth, 1);
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
