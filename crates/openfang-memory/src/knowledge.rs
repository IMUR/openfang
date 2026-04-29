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
use openfang_types::agent::AgentId;
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory::{
    Entity, EntityType, GraphMatch, GraphPattern, Relation, RelationType,
};
use openfang_types::memory_dimensions::{
    MemoryContract, MemoryDataModel, MemoryIntelligence, MemorySubstrateKind, MemorySurfaceSpec,
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

/// Implement `SurrealValue` for a type via a serde-json round-trip.
///
/// Used for structs whose fields contain custom openfang-types (EntityType, RelationType,
/// Message, etc.) that do not themselves implement `SurrealValue`. The serde-json bridge
/// preserves full fidelity because SurrealDB stores these fields as FLEXIBLE TYPE any.
macro_rules! surreal_via_json {
    ($t:ty) => {
        impl surrealdb::types::SurrealValue for $t {
            fn kind_of() -> surrealdb::types::Kind {
                surrealdb::types::Kind::Any
            }
            fn into_value(self) -> surrealdb::types::Value {
                let json = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
                surrealdb::types::SurrealValue::into_value(json)
            }
            fn from_value(value: surrealdb::types::Value) -> Result<Self, surrealdb::types::Error> {
                let json = value.into_json_value();
                serde_json::from_value(json)
                    .map_err(|e| surrealdb::types::Error::internal(e.to_string()))
            }
        }
    };
}

/// Knowledge graph store backed by SurrealDB.
#[derive(Clone)]
pub struct KnowledgeStore {
    db: SurrealDb,
}

pub const KNOWLEDGE_GRAPH_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "entities_relations",
    description: "Agent-scoped knowledge graph entities and relation edges",
    storage_tables: &["entities", "relations"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Graph, MemoryDataModel::Document],
    contracts: &[
        MemoryContract::Context,
        MemoryContract::Tool,
        MemoryContract::Operations,
    ],
    intelligence: &[MemoryIntelligence::Ner],
};

/// Entity record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct EntityRecord {
    #[serde(default)]
    agent_id: Option<String>,
    entity_type: EntityType,
    name: String,
    properties: std::collections::HashMap<String, serde_json::Value>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    updated_at: String,
}
surreal_via_json!(EntityRecord);

/// Entity row returned by read-only inspection queries.
#[derive(Debug, Serialize, Deserialize)]
struct EntityRow {
    record_key: Option<String>,
    #[serde(default)]
    agent_id: Option<String>,
    entity_type: EntityType,
    name: String,
    properties: std::collections::HashMap<String, serde_json::Value>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    updated_at: String,
}
surreal_via_json!(EntityRow);

/// Relation record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct RelationRecord {
    #[serde(default)]
    agent_id: Option<String>,
    relation_type: RelationType,
    properties: std::collections::HashMap<String, serde_json::Value>,
    confidence: f32,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
}
surreal_via_json!(RelationRecord);

/// Raw graph query result row.
#[derive(Debug, Serialize, Deserialize)]
struct GraphRow {
    source: EntityRecord,
    relation: RelationRecord,
    target: EntityRecord,
    source_id: String,
    target_id: String,
    relation_source: String,
    relation_target: String,
}
surreal_via_json!(GraphRow);

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

fn parse_dt(s: &str) -> chrono::DateTime<Utc> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now())
}

fn parse_agent_id(value: Option<String>) -> Option<AgentId> {
    value
        .and_then(|id| uuid::Uuid::parse_str(&id).ok())
        .map(AgentId)
}

fn agent_id_string(agent_id: Option<AgentId>) -> Option<String> {
    agent_id.map(|id| id.0.to_string())
}

fn entity_row_to_entity(row: EntityRow) -> Entity {
    Entity {
        id: row.record_key.unwrap_or_default(),
        agent_id: parse_agent_id(row.agent_id),
        entity_type: row.entity_type,
        name: row.name,
        properties: row.properties,
        created_at: parse_dt(&row.created_at),
        updated_at: parse_dt(&row.updated_at),
    }
}

fn graph_row_to_match(row: GraphRow) -> GraphMatch {
    GraphMatch {
        source: Entity {
            id: row.source_id,
            agent_id: parse_agent_id(row.source.agent_id),
            entity_type: row.source.entity_type,
            name: row.source.name,
            properties: row.source.properties,
            created_at: parse_dt(&row.source.created_at),
            updated_at: parse_dt(&row.source.updated_at),
        },
        relation: Relation {
            agent_id: parse_agent_id(row.relation.agent_id),
            source: row.relation_source,
            relation: row.relation.relation_type,
            target: row.relation_target,
            properties: row.relation.properties,
            confidence: row.relation.confidence,
            created_at: parse_dt(&row.relation.created_at),
        },
        target: Entity {
            id: row.target_id,
            agent_id: parse_agent_id(row.target.agent_id),
            entity_type: row.target.entity_type,
            name: row.target.name,
            properties: row.target.properties,
            created_at: parse_dt(&row.target.created_at),
            updated_at: parse_dt(&row.target.updated_at),
        },
    }
}

// ---------------------------------------------------------------------------
// Traversal helper types and functions
// ---------------------------------------------------------------------------

/// Raw entity record as returned by a graph traversal hop.
#[derive(Debug, Serialize, Deserialize)]
struct RawEntity {
    /// Record ID as returned by v3 graph expansion — string like "entities:acme".
    #[serde(rename = "id")]
    raw_id: Option<serde_json::Value>,
    #[serde(default)]
    agent_id: Option<String>,
    entity_type: Option<EntityType>,
    name: Option<String>,
    #[serde(default)]
    properties: std::collections::HashMap<String, serde_json::Value>,
    #[serde(
        default,
        deserialize_with = "openfang_types::datetime::deserialize_optional_rfc3339_string"
    )]
    created_at: Option<String>,
    #[serde(
        default,
        deserialize_with = "openfang_types::datetime::deserialize_optional_rfc3339_string"
    )]
    updated_at: Option<String>,
}
surreal_via_json!(RawEntity);

/// Result row from the multi-hop traversal query.
#[derive(Debug, Serialize, Deserialize)]
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
surreal_via_json!(TraversalResult);

/// Convert a batch of raw entities from one hop into `TraversalNode`s, deduplicating.
fn collect_hop(
    raw_ents: Vec<RawEntity>,
    hop_depth: usize,
    via: &str,
    seen: &mut std::collections::HashSet<String>,
    nodes: &mut Vec<TraversalNode>,
) {
    for re in raw_ents {
        // raw_id is a string like "entities:acme" or a RecordId object — extract the key part.
        let eid = re
            .raw_id
            .as_ref()
            .and_then(|v| match v {
                serde_json::Value::String(s) => {
                    // "entities:acme" → "acme"
                    s.split_once(':')
                        .map(|(_, key)| key.to_string())
                        .or_else(|| Some(s.clone()))
                }
                _ => Some(format!("{v}")),
            })
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
            agent_id: parse_agent_id(re.agent_id),
            entity_type: re
                .entity_type
                .unwrap_or(EntityType::Custom("unknown".to_string())),
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
                agent_id: agent_id_string(entity.agent_id),
                entity_type: entity.entity_type,
                name: entity.name,
                properties: entity.properties,
                created_at: now.clone(),
                updated_at: now.clone(),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('entities', $id)
                 SET created_at = type::datetime($created_at),
                     updated_at = type::datetime($updated_at)",
            )
            .bind(("id", id.clone()))
            .bind(("created_at", now.clone()))
            .bind(("updated_at", now))
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
                "RELATE (type::record('entities', $source))->(type::record('relations', $id))->(type::record('entities', $target))
                 SET relation_type = $rel_type,
                     agent_id = $agent_id,
                     properties = $props,
                     confidence = $conf,
                     created_at = $now"
            )
            .bind(("source", relation.source.clone()))
            .bind(("target", relation.target.clone()))
            .bind(("id", id.clone()))
            .bind(("agent_id", agent_id_string(relation.agent_id)))
            .bind(("rel_type", serde_json::to_value(&relation.relation).map_err(|e| OpenFangError::Serialization(e.to_string()))?))
            .bind(("props", serde_json::to_value(&relation.properties).map_err(|e| OpenFangError::Serialization(e.to_string()))?))
            .bind(("conf", relation.confidence as f64))
            .bind(("now", now.clone()))
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('relations', $id)
                 SET created_at = type::datetime($created_at)",
            )
            .bind(("id", id.clone()))
            .bind(("created_at", now))
            .await
            .map_err(surreal_err)?;

        Ok(id)
    }

    /// Query the knowledge graph with a pattern.
    pub async fn query_graph(&self, pattern: GraphPattern) -> OpenFangResult<Vec<GraphMatch>> {
        // Build a dynamic SurrealQL query with optional filters
        let mut conditions = Vec::new();
        let mut bindings: Vec<(String, serde_json::Value)> = Vec::new();

        if let Some(agent_id) = pattern.agent_id {
            conditions.push("agent_id = $agent_id");
            bindings.push(("agent_id".into(), serde_json::json!(agent_id.0.to_string())));
        }
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
                {{ agent_id: agent_id, relation_type: relation_type, properties: properties,
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

        let matches = rows.into_iter().map(graph_row_to_match).collect();

        Ok(matches)
    }

    /// List entities for read-only graph inspection.
    pub async fn list_entities(&self, offset: usize, limit: usize) -> OpenFangResult<Vec<Entity>> {
        let sql = "SELECT meta::id(id) AS record_key, entity_type, name, properties, created_at, updated_at
                   FROM entities
                   ORDER BY updated_at DESC, created_at DESC
                   LIMIT $lim START $off";

        let mut result = self
            .db
            .query(sql)
            .bind(("lim", limit))
            .bind(("off", offset))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<EntityRow> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(entity_row_to_entity).collect())
    }

    /// List entities owned by one agent for read-only graph inspection.
    pub async fn list_entities_for_agent(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<Entity>> {
        let sql = "SELECT meta::id(id) AS record_key, agent_id, entity_type, name, properties, created_at, updated_at
                   FROM entities
                   WHERE agent_id = $agent_id
                   ORDER BY updated_at DESC, created_at DESC
                   LIMIT $lim START $off";

        let mut result = self
            .db
            .query(sql)
            .bind(("agent_id", agent_id.0.to_string()))
            .bind(("lim", limit))
            .bind(("off", offset))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<EntityRow> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(entity_row_to_entity).collect())
    }

    /// List graph relations for read-only graph inspection.
    pub async fn list_relations(
        &self,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<GraphMatch>> {
        let sql = "SELECT
                      in.* AS source,
                      out.* AS target,
                      { agent_id: agent_id, relation_type: relation_type, properties: properties,
                        confidence: confidence, created_at: created_at } AS relation,
                      meta::id(in) AS source_id,
                      meta::id(out) AS target_id,
                      meta::id(in) AS relation_source,
                      meta::id(out) AS relation_target,
                      created_at AS relation_created_at
                   FROM `relations`
                   ORDER BY created_at DESC
                   LIMIT $lim START $off";

        let mut result = self
            .db
            .query(sql)
            .bind(("lim", limit))
            .bind(("off", offset))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<GraphRow> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(graph_row_to_match).collect())
    }

    /// List relations owned by one agent for read-only graph inspection.
    pub async fn list_relations_for_agent(
        &self,
        agent_id: AgentId,
        offset: usize,
        limit: usize,
    ) -> OpenFangResult<Vec<GraphMatch>> {
        let sql = "SELECT
                      in.* AS source,
                      out.* AS target,
                      { agent_id: agent_id, relation_type: relation_type, properties: properties,
                        confidence: confidence, created_at: created_at } AS relation,
                      meta::id(in) AS source_id,
                      meta::id(out) AS target_id,
                      meta::id(in) AS relation_source,
                      meta::id(out) AS relation_target,
                      created_at AS relation_created_at
                   FROM `relations`
                   WHERE agent_id = $agent_id
                   ORDER BY created_at DESC
                   LIMIT $lim START $off";

        let mut result = self
            .db
            .query(sql)
            .bind(("agent_id", agent_id.0.to_string()))
            .bind(("lim", limit))
            .bind(("off", offset))
            .await
            .map_err(surreal_err)?;
        let rows: Vec<GraphRow> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().map(graph_row_to_match).collect())
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
            1 => "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1
                 FROM type::record('entities', $src)"
                .to_string(),
            2 => "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1,
                        (->relations->entities->relations->entities.*) AS hop2
                 FROM type::record('entities', $src)"
                .to_string(),
            _ => "SELECT meta::id(id) AS eid,
                        (->relations->entities.*) AS hop1,
                        (->relations->entities->relations->entities.*) AS hop2,
                        (->relations->entities->relations->entities->relations->entities.*) AS hop3
                 FROM type::record('entities', $src)"
                .to_string(),
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

    /// Multi-hop traversal constrained to relations owned by one agent.
    pub async fn traverse_from_for_agent(
        &self,
        source_id: &str,
        agent_id: AgentId,
        max_depth: usize,
    ) -> OpenFangResult<Vec<TraversalNode>> {
        let depth = max_depth.clamp(1, 3);
        let mut nodes = Vec::new();
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        let mut frontier = vec![source_id.to_string()];
        seen.insert(source_id.to_string());

        for hop_depth in 1..=depth {
            let mut next_frontier = Vec::new();
            for current in &frontier {
                let matches = self
                    .query_graph(GraphPattern {
                        agent_id: Some(agent_id),
                        source: Some(current.clone()),
                        relation: None,
                        target: None,
                        max_depth: 1,
                    })
                    .await?;
                for graph_match in matches {
                    let target_id = graph_match.target.id.clone();
                    if target_id.is_empty() || !seen.insert(target_id.clone()) {
                        continue;
                    }
                    next_frontier.push(target_id);
                    nodes.push(TraversalNode {
                        entity: graph_match.target,
                        depth: hop_depth,
                        via_entity_id: current.clone(),
                    });
                }
            }
            if next_frontier.is_empty() {
                break;
            }
            frontier = next_frontier;
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
                agent_id: None,
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
                agent_id: None,
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
                agent_id: None,
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
                agent_id: None,
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
    async fn test_agent_scoped_graph_queries_do_not_cross_agents() {
        let store = setup().await;
        let agent_a = openfang_types::agent::AgentId::new();
        let agent_b = openfang_types::agent::AgentId::new();

        let alice_a = store
            .add_entity(Entity {
                id: "alice-a".to_string(),
                agent_id: Some(agent_a),
                entity_type: EntityType::Person,
                name: "Alice".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let acme_a = store
            .add_entity(Entity {
                id: "acme-a".to_string(),
                agent_id: Some(agent_a),
                entity_type: EntityType::Organization,
                name: "Acme A".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let alice_b = store
            .add_entity(Entity {
                id: "alice-b".to_string(),
                agent_id: Some(agent_b),
                entity_type: EntityType::Person,
                name: "Alice".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();
        let acme_b = store
            .add_entity(Entity {
                id: "acme-b".to_string(),
                agent_id: Some(agent_b),
                entity_type: EntityType::Organization,
                name: "Acme B".to_string(),
                properties: HashMap::new(),
                created_at: Utc::now(),
                updated_at: Utc::now(),
            })
            .await
            .unwrap();

        store
            .add_relation(Relation {
                agent_id: Some(agent_a),
                source: alice_a.clone(),
                relation: RelationType::WorksAt,
                target: acme_a.clone(),
                properties: HashMap::new(),
                confidence: 0.9,
                created_at: Utc::now(),
            })
            .await
            .unwrap();
        store
            .add_relation(Relation {
                agent_id: Some(agent_b),
                source: alice_b,
                relation: RelationType::WorksAt,
                target: acme_b,
                properties: HashMap::new(),
                confidence: 0.9,
                created_at: Utc::now(),
            })
            .await
            .unwrap();

        let entities = store.list_entities_for_agent(agent_a, 0, 10).await.unwrap();
        let entity_names: Vec<&str> = entities.iter().map(|entity| entity.name.as_str()).collect();
        assert_eq!(entity_names.len(), 2);
        assert!(entity_names.contains(&"Acme A"));
        assert!(!entity_names.contains(&"Acme B"));

        let matches = store
            .query_graph(GraphPattern {
                agent_id: Some(agent_a),
                source: Some("Alice".to_string()),
                relation: Some(RelationType::WorksAt),
                target: None,
                max_depth: 1,
            })
            .await
            .unwrap();
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].target.name, "Acme A");

        let nodes = store
            .traverse_from_for_agent(&alice_a, agent_a, 1)
            .await
            .unwrap();
        let traversal_names: Vec<&str> =
            nodes.iter().map(|node| node.entity.name.as_str()).collect();
        assert!(traversal_names.contains(&"Acme A"));
        assert!(!traversal_names.contains(&"Acme B"));
    }

    #[tokio::test]
    async fn test_add_relation() {
        let store = setup().await;
        let alice_id = store
            .add_entity(Entity {
                id: "alice".to_string(),
                agent_id: None,
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
                agent_id: None,
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
                agent_id: None,
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
