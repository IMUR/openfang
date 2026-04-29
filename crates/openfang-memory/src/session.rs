//! Session management — load/save conversation history via SurrealDB.
//!
//! Sessions are stored as documents. Messages are serialized as JSON arrays
//! directly (no msgpack blobs). Canonical sessions maintain cross-channel
//! context with compaction.

use chrono::Utc;
use openfang_types::agent::{AgentId, SessionId};
use openfang_types::error::{OpenFangError, OpenFangResult};
use openfang_types::memory_dimensions::{
    MemoryContract, MemoryDataModel, MemoryIntelligence, MemorySubstrateKind, MemorySurfaceSpec,
};
use openfang_types::message::{ContentBlock, Message, MessageContent, Role};
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::db::SurrealDb;

pub const SESSION_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "sessions",
    description: "Conversation sessions and canonical compacted session context",
    storage_tables: &["sessions", "canonical_sessions"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Document],
    contracts: &[MemoryContract::Context, MemoryContract::Operations],
    intelligence: &[MemoryIntelligence::Summarizer],
};

pub const TRANSCRIPT_ARCHIVE_SURFACE: MemorySurfaceSpec = MemorySurfaceSpec {
    id: "transcript_archives",
    description: "Durable transcript snapshots retained before active session compaction",
    storage_tables: &["transcript_archives"],
    substrates: &[MemorySubstrateKind::SurrealDb],
    data_models: &[MemoryDataModel::Document],
    contracts: &[MemoryContract::Operations, MemoryContract::Context],
    intelligence: &[MemoryIntelligence::None],
};

/// Implement `SurrealValue` via serde-json round-trip for types that contain
/// `openfang-types` structs (e.g. `Message`) which do not implement `SurrealValue`.
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

/// A conversation session with message history.
#[derive(Debug, Clone)]
pub struct Session {
    /// Session ID.
    pub id: SessionId,
    /// Owning agent ID.
    pub agent_id: AgentId,
    /// Conversation messages.
    pub messages: Vec<Message>,
    /// Estimated token count for the context window.
    pub context_window_tokens: u64,
    /// Optional human-readable session label.
    pub label: Option<String>,
}

/// Session record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct SessionRecord {
    agent_id: String,
    messages: Vec<Message>,
    context_window_tokens: u64,
    label: Option<String>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    updated_at: String,
}
surreal_via_json!(SessionRecord);

/// Canonical session record for SurrealDB persistence.
#[derive(Debug, Serialize, Deserialize)]
struct CanonicalRecord {
    agent_id: String,
    messages: Vec<Message>,
    compaction_cursor: usize,
    compacted_summary: Option<String>,
    #[serde(default)]
    rolling_summary: Option<String>,
    #[serde(default)]
    rolling_summary_cursor: usize,
    #[serde(
        default,
        deserialize_with = "openfang_types::datetime::deserialize_optional_rfc3339_string"
    )]
    rolling_summary_updated_at: Option<String>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    updated_at: String,
}
surreal_via_json!(CanonicalRecord);

#[derive(Debug, Serialize, Deserialize)]
struct TranscriptArchiveRecord {
    agent_id: String,
    session_id: String,
    reason: String,
    messages: Vec<Message>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    archived_at: String,
}
surreal_via_json!(TranscriptArchiveRecord);

/// Session listing row.
#[derive(Debug, Serialize, Deserialize)]
struct SessionListRow {
    id: serde_json::Value,
    agent_id: String,
    messages: Vec<Message>,
    #[serde(deserialize_with = "openfang_types::datetime::deserialize_rfc3339_string")]
    created_at: String,
    label: Option<String>,
}
surreal_via_json!(SessionListRow);

fn surreal_err(e: surrealdb::Error) -> OpenFangError {
    OpenFangError::Memory(e.to_string())
}

/// Session store backed by SurrealDB.
#[derive(Clone)]
pub struct SessionStore {
    db: SurrealDb,
}

impl SessionStore {
    /// Create a new session store wrapping the given SurrealDB handle.
    pub fn new(db: SurrealDb) -> Self {
        Self { db }
    }

    /// Load a session from the database.
    pub async fn get_session(&self, session_id: SessionId) -> OpenFangResult<Option<Session>> {
        let result: Option<SessionRecord> = self
            .db
            .select(("sessions", session_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;

        Ok(result.map(|r| {
            let agent_id = uuid::Uuid::parse_str(&r.agent_id)
                .map(AgentId)
                .unwrap_or_else(|_| AgentId::new());
            Session {
                id: session_id,
                agent_id,
                messages: r.messages,
                context_window_tokens: r.context_window_tokens,
                label: r.label,
            }
        }))
    }

    /// Save a session to the database.
    pub async fn save_session(&self, session: &Session) -> OpenFangResult<()> {
        let now = Utc::now().to_rfc3339();
        let _: Option<SessionRecord> = self
            .db
            .upsert(("sessions", session.id.0.to_string().as_str()))
            .content(SessionRecord {
                agent_id: session.agent_id.0.to_string(),
                messages: session.messages.clone(),
                context_window_tokens: session.context_window_tokens,
                label: session.label.clone(),
                created_at: now.clone(),
                updated_at: now.clone(),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('sessions', $sid)
                 SET created_at = type::datetime($created_at),
                     updated_at = type::datetime($updated_at)",
            )
            .bind(("sid", session.id.0.to_string()))
            .bind(("created_at", now.clone()))
            .bind(("updated_at", now))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Delete a session from the database.
    pub async fn delete_session(&self, session_id: SessionId) -> OpenFangResult<()> {
        let _: Option<SessionRecord> = self
            .db
            .delete(("sessions", session_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Delete all sessions belonging to an agent.
    pub async fn delete_agent_sessions(&self, agent_id: AgentId) -> OpenFangResult<()> {
        self.db
            .query("DELETE sessions WHERE agent_id = $aid")
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Delete the canonical (cross-channel) session for an agent.
    pub async fn delete_canonical_session(&self, agent_id: AgentId) -> OpenFangResult<()> {
        let _: Option<CanonicalRecord> = self
            .db
            .delete(("canonical_sessions", agent_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// List all sessions with metadata (session_id, agent_id, message_count, created_at).
    pub async fn list_sessions(&self) -> OpenFangResult<Vec<serde_json::Value>> {
        let mut result = self
            .db
            .query("SELECT * FROM sessions ORDER BY created_at DESC")
            .await
            .map_err(surreal_err)?;

        let rows: Vec<SessionListRow> = result.take(0).unwrap_or_default();
        Ok(rows
            .into_iter()
            .map(|r| {
                let id_str = match &r.id {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                serde_json::json!({
                    "session_id": id_str,
                    "agent_id": r.agent_id,
                    "message_count": r.messages.len(),
                    "created_at": r.created_at,
                    "label": r.label,
                })
            })
            .collect())
    }

    /// Create a new empty session for an agent.
    pub async fn create_session(&self, agent_id: AgentId) -> OpenFangResult<Session> {
        let session = Session {
            id: SessionId::new(),
            agent_id,
            messages: Vec::new(),
            context_window_tokens: 0,
            label: None,
        };
        self.save_session(&session).await?;
        Ok(session)
    }

    /// Set the label on an existing session.
    pub async fn set_session_label(
        &self,
        session_id: SessionId,
        label: Option<&str>,
    ) -> OpenFangResult<()> {
        self.db
            .query(
                "UPDATE type::record('sessions', $sid)
                 SET label = $label, updated_at = time::now()",
            )
            .bind(("sid", session_id.0.to_string()))
            .bind(("label", label.map(|s| s.to_string())))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }

    /// Find a session by label for a given agent.
    pub async fn find_session_by_label(
        &self,
        agent_id: AgentId,
        label: &str,
    ) -> OpenFangResult<Option<Session>> {
        let mut result = self
            .db
            .query("SELECT * FROM sessions WHERE agent_id = $aid AND label = $label LIMIT 1")
            .bind(("aid", agent_id.0.to_string()))
            .bind(("label", label.to_string()))
            .await
            .map_err(surreal_err)?;

        let rows: Vec<SessionRecord> = result.take(0).unwrap_or_default();
        Ok(rows.into_iter().next().map(|r| {
            let sid = uuid::Uuid::parse_str(&r.agent_id)
                .map(SessionId)
                .unwrap_or_else(|_| SessionId::new());
            Session {
                id: sid,
                agent_id,
                messages: r.messages,
                context_window_tokens: r.context_window_tokens,
                label: r.label,
            }
        }))
    }

    /// List all sessions for a specific agent.
    pub async fn list_agent_sessions(
        &self,
        agent_id: AgentId,
    ) -> OpenFangResult<Vec<serde_json::Value>> {
        let mut result = self
            .db
            .query("SELECT * FROM sessions WHERE agent_id = $aid ORDER BY created_at DESC")
            .bind(("aid", agent_id.0.to_string()))
            .await
            .map_err(surreal_err)?;

        let rows: Vec<SessionListRow> = result.take(0).unwrap_or_default();
        Ok(rows
            .into_iter()
            .map(|r| {
                let id_str = match &r.id {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                serde_json::json!({
                    "session_id": id_str,
                    "message_count": r.messages.len(),
                    "created_at": r.created_at,
                    "label": r.label,
                })
            })
            .collect())
    }

    /// Create a new session with an optional label.
    pub async fn create_session_with_label(
        &self,
        agent_id: AgentId,
        label: Option<&str>,
    ) -> OpenFangResult<Session> {
        let session = Session {
            id: SessionId::new(),
            agent_id,
            messages: Vec::new(),
            context_window_tokens: 0,
            label: label.map(|s| s.to_string()),
        };
        self.save_session(&session).await?;
        Ok(session)
    }

    /// Store an LLM-generated summary, replacing older messages with the summary
    /// and keeping only the specified recent messages.
    pub async fn store_llm_summary(
        &self,
        agent_id: AgentId,
        summary: &str,
        kept_messages: Vec<Message>,
    ) -> OpenFangResult<()> {
        let mut canonical = self.load_canonical(agent_id).await?;
        canonical.compacted_summary = Some(summary.to_string());
        canonical.messages = kept_messages;
        canonical.compaction_cursor = 0;
        canonical.updated_at = Utc::now().to_rfc3339();
        self.save_canonical(&canonical).await
    }

    /// Archive a full transcript snapshot before active session compaction.
    pub async fn archive_transcript(
        &self,
        agent_id: AgentId,
        session_id: SessionId,
        messages: &[Message],
        reason: &str,
    ) -> OpenFangResult<String> {
        let archive_id = uuid::Uuid::new_v4().to_string();
        let now = Utc::now().to_rfc3339();
        let _: Option<TranscriptArchiveRecord> = self
            .db
            .upsert(("transcript_archives", archive_id.as_str()))
            .content(TranscriptArchiveRecord {
                agent_id: agent_id.0.to_string(),
                session_id: session_id.0.to_string(),
                reason: reason.to_string(),
                messages: messages.to_vec(),
                archived_at: now.clone(),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('transcript_archives', $id)
                 SET archived_at = type::datetime($archived_at)",
            )
            .bind(("id", archive_id.clone()))
            .bind(("archived_at", now))
            .await
            .map_err(surreal_err)?;
        Ok(archive_id)
    }

    /// Return the most recent transcript archive for a session.
    pub async fn latest_transcript_archive(
        &self,
        session_id: SessionId,
    ) -> OpenFangResult<Option<Vec<Message>>> {
        let mut result = self
            .db
            .query(
                "SELECT agent_id, session_id, reason, messages, archived_at
                 FROM transcript_archives
                 WHERE session_id = $session_id
                 ORDER BY archived_at DESC
                 LIMIT 1",
            )
            .bind(("session_id", session_id.0.to_string()))
            .await
            .map_err(surreal_err)?;
        let records: Vec<TranscriptArchiveRecord> = result.take(0).unwrap_or_default();
        Ok(records.into_iter().next().map(|record| record.messages))
    }
}

/// Default number of recent messages to include from canonical session.
const DEFAULT_CANONICAL_WINDOW: usize = 50;

/// Default compaction threshold: when message count exceeds this, compact older messages.
const DEFAULT_COMPACTION_THRESHOLD: usize = 100;

/// A canonical session stores persistent cross-channel context for an agent.
///
/// Unlike regular sessions (one per channel interaction), there is one canonical
/// session per agent. All channels contribute to it, so what a user tells an agent
/// on Telegram is remembered on Discord.
#[derive(Debug, Clone)]
pub struct CanonicalSession {
    /// The agent this session belongs to.
    pub agent_id: AgentId,
    /// Full message history (post-compaction window).
    pub messages: Vec<Message>,
    /// Index marking how far compaction has processed.
    pub compaction_cursor: usize,
    /// Summary of compacted (older) messages.
    pub compacted_summary: Option<String>,
    pub rolling_summary: Option<String>,
    pub rolling_summary_cursor: usize,
    pub rolling_summary_updated_at: Option<String>,
    /// Last update time.
    pub updated_at: String,
}

impl SessionStore {
    /// Load the canonical session for an agent, creating one if it doesn't exist.
    pub async fn load_canonical(&self, agent_id: AgentId) -> OpenFangResult<CanonicalSession> {
        let result: Option<CanonicalRecord> = self
            .db
            .select(("canonical_sessions", agent_id.0.to_string().as_str()))
            .await
            .map_err(surreal_err)?;

        match result {
            Some(r) => Ok(CanonicalSession {
                agent_id,
                messages: r.messages,
                compaction_cursor: r.compaction_cursor,
                compacted_summary: r.compacted_summary,
                rolling_summary: r.rolling_summary,
                rolling_summary_cursor: r.rolling_summary_cursor,
                rolling_summary_updated_at: r.rolling_summary_updated_at,
                updated_at: r.updated_at,
            }),
            None => {
                let now = Utc::now().to_rfc3339();
                Ok(CanonicalSession {
                    agent_id,
                    messages: Vec::new(),
                    compaction_cursor: 0,
                    compacted_summary: None,
                    rolling_summary: None,
                    rolling_summary_cursor: 0,
                    rolling_summary_updated_at: None,
                    updated_at: now,
                })
            }
        }
    }

    /// Append new messages to the canonical session and compact if over threshold.
    pub async fn append_canonical(
        &self,
        agent_id: AgentId,
        new_messages: &[Message],
        compaction_threshold: Option<usize>,
    ) -> OpenFangResult<CanonicalSession> {
        let mut canonical = self.load_canonical(agent_id).await?;
        canonical.messages.extend(new_messages.iter().cloned());

        let threshold = compaction_threshold.unwrap_or(DEFAULT_COMPACTION_THRESHOLD);

        // Compact if over threshold
        if canonical.messages.len() > threshold {
            let keep_count = DEFAULT_CANONICAL_WINDOW;
            let to_compact = canonical.messages.len().saturating_sub(keep_count);
            if to_compact > canonical.compaction_cursor {
                let compacting = &canonical.messages[canonical.compaction_cursor..to_compact];
                let mut summary_parts: Vec<String> = Vec::new();
                if let Some(ref existing) = canonical.compacted_summary {
                    summary_parts.push(existing.clone());
                }
                for msg in compacting {
                    let role = match msg.role {
                        Role::User => "User",
                        Role::Assistant => "Assistant",
                        Role::System => "System",
                    };
                    let text = msg.content.text_content();
                    if !text.is_empty() {
                        let truncated = if text.len() > 200 {
                            format!("{}...", openfang_types::truncate_str(&text, 200))
                        } else {
                            text
                        };
                        summary_parts.push(format!("{role}: {truncated}"));
                    }
                }
                let mut full_summary = summary_parts.join("\n");
                if full_summary.len() > 4000 {
                    let start = full_summary.len() - 4000;
                    let safe_start = (start..full_summary.len())
                        .find(|&i| full_summary.is_char_boundary(i))
                        .unwrap_or(full_summary.len());
                    full_summary = full_summary[safe_start..].to_string();
                }
                canonical.compacted_summary = Some(full_summary);
                canonical.compaction_cursor = to_compact;
                canonical.messages = canonical.messages.split_off(to_compact);
                canonical.compaction_cursor = 0;
            }
        }

        canonical.updated_at = Utc::now().to_rfc3339();
        self.save_canonical(&canonical).await?;
        Ok(canonical)
    }

    /// Get recent messages from canonical session for context injection.
    pub async fn canonical_context(
        &self,
        agent_id: AgentId,
        window_size: Option<usize>,
    ) -> OpenFangResult<(Option<String>, Vec<Message>)> {
        let canonical = self.load_canonical(agent_id).await?;
        let window = window_size.unwrap_or(DEFAULT_CANONICAL_WINDOW);
        let start = canonical.messages.len().saturating_sub(window);
        let recent = canonical.messages[start..].to_vec();
        Ok((
            canonical
                .rolling_summary
                .clone()
                .or(canonical.compacted_summary.clone()),
            recent,
        ))
    }

    pub async fn rolling_context(
        &self,
        agent_id: AgentId,
    ) -> OpenFangResult<(Option<String>, usize, usize)> {
        let canonical = self.load_canonical(agent_id).await?;
        Ok((
            canonical.rolling_summary,
            canonical.rolling_summary_cursor,
            canonical.messages.len(),
        ))
    }

    pub async fn canonical_messages(&self, agent_id: AgentId) -> OpenFangResult<Vec<Message>> {
        Ok(self.load_canonical(agent_id).await?.messages)
    }

    pub async fn update_rolling_summary(
        &self,
        agent_id: AgentId,
        summary: &str,
        cursor: usize,
    ) -> OpenFangResult<()> {
        let mut canonical = self.load_canonical(agent_id).await?;
        canonical.rolling_summary = Some(summary.to_string());
        canonical.rolling_summary_cursor = cursor.min(canonical.messages.len());
        canonical.rolling_summary_updated_at = Some(Utc::now().to_rfc3339());
        canonical.updated_at = Utc::now().to_rfc3339();
        self.save_canonical(&canonical).await
    }

    /// Persist a canonical session to SurrealDB.
    async fn save_canonical(&self, canonical: &CanonicalSession) -> OpenFangResult<()> {
        let _: Option<CanonicalRecord> = self
            .db
            .upsert((
                "canonical_sessions",
                canonical.agent_id.0.to_string().as_str(),
            ))
            .content(CanonicalRecord {
                agent_id: canonical.agent_id.0.to_string(),
                messages: canonical.messages.clone(),
                compaction_cursor: canonical.compaction_cursor,
                compacted_summary: canonical.compacted_summary.clone(),
                rolling_summary: canonical.rolling_summary.clone(),
                rolling_summary_cursor: canonical.rolling_summary_cursor,
                rolling_summary_updated_at: canonical.rolling_summary_updated_at.clone(),
                updated_at: canonical.updated_at.clone(),
            })
            .await
            .map_err(surreal_err)?;
        self.db
            .query(
                "UPDATE type::record('canonical_sessions', $aid)
                 SET updated_at = type::datetime($updated_at)",
            )
            .bind(("aid", canonical.agent_id.0.to_string()))
            .bind(("updated_at", canonical.updated_at.clone()))
            .await
            .map_err(surreal_err)?;
        Ok(())
    }
}

/// A single JSONL line in the session mirror file.
#[derive(Serialize)]
struct JsonlLine {
    timestamp: String,
    role: String,
    content: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_use: Option<serde_json::Value>,
}

impl SessionStore {
    /// Write a human-readable JSONL mirror of a session to disk.
    ///
    /// Best-effort: errors are returned but should be logged and never
    /// affect the primary SurrealDB store.
    pub async fn write_jsonl_mirror(
        &self,
        session: Session,
        sessions_dir: PathBuf,
    ) -> Result<(), std::io::Error> {
        tokio::task::spawn_blocking(move || write_jsonl_mirror_sync(&session, &sessions_dir))
            .await
            .map_err(std::io::Error::other)?
    }
}

fn write_jsonl_mirror_sync(session: &Session, sessions_dir: &Path) -> Result<(), std::io::Error> {
    std::fs::create_dir_all(sessions_dir)?;
    let path = sessions_dir.join(format!("{}.jsonl", session.id.0));
    let mut file = std::fs::File::create(&path)?;
    let now = Utc::now().to_rfc3339();

    for msg in &session.messages {
        let role_str = match msg.role {
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::System => "system",
        };

        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_parts: Vec<serde_json::Value> = Vec::new();

        match &msg.content {
            MessageContent::Text(t) => {
                text_parts.push(t.clone());
            }
            MessageContent::Blocks(blocks) => {
                for block in blocks {
                    match block {
                        ContentBlock::Text { text, .. } => {
                            text_parts.push(text.clone());
                        }
                        ContentBlock::ToolUse {
                            id, name, input, ..
                        } => {
                            tool_parts.push(serde_json::json!({
                                "type": "tool_use",
                                "id": id,
                                "name": name,
                                "input": input,
                            }));
                        }
                        ContentBlock::ToolResult {
                            tool_use_id,
                            tool_name: _,
                            content,
                            is_error,
                        } => {
                            tool_parts.push(serde_json::json!({
                                "type": "tool_result",
                                "tool_use_id": tool_use_id,
                                "content": content,
                                "is_error": is_error,
                            }));
                        }
                        ContentBlock::Image { media_type, .. } => {
                            text_parts.push(format!("[image: {media_type}]"));
                        }
                        ContentBlock::Thinking { thinking } => {
                            text_parts.push(format!(
                                "[thinking: {}]",
                                openfang_types::truncate_str(thinking, 200)
                            ));
                        }
                        ContentBlock::Unknown => {}
                    }
                }
            }
        }

        let line = JsonlLine {
            timestamp: now.clone(),
            role: role_str.to_string(),
            content: serde_json::Value::String(text_parts.join("\n")),
            tool_use: if tool_parts.is_empty() {
                None
            } else {
                Some(serde_json::Value::Array(tool_parts))
            },
        };

        serde_json::to_writer(&mut file, &line).map_err(std::io::Error::other)?;
        file.write_all(b"\n")?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;

    async fn setup() -> SessionStore {
        let db = db::init_mem().await.unwrap();
        SessionStore::new(db)
    }

    #[tokio::test]
    async fn test_create_and_load_session() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let session = store.create_session(agent_id).await.unwrap();

        let loaded = store.get_session(session.id).await.unwrap().unwrap();
        assert_eq!(loaded.agent_id, agent_id);
        assert!(loaded.messages.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_load_with_messages() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let mut session = store.create_session(agent_id).await.unwrap();
        session.messages.push(Message::user("Hello"));
        session.messages.push(Message::assistant("Hi there!"));
        store.save_session(&session).await.unwrap();

        let loaded = store.get_session(session.id).await.unwrap().unwrap();
        assert_eq!(loaded.messages.len(), 2);
    }

    #[tokio::test]
    async fn test_get_missing_session() {
        let store = setup().await;
        let result = store.get_session(SessionId::new()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_delete_session() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let session = store.create_session(agent_id).await.unwrap();
        let sid = session.id;
        assert!(store.get_session(sid).await.unwrap().is_some());
        store.delete_session(sid).await.unwrap();
        assert!(store.get_session(sid).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_agent_sessions() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let s1 = store.create_session(agent_id).await.unwrap();
        let s2 = store.create_session(agent_id).await.unwrap();
        assert!(store.get_session(s1.id).await.unwrap().is_some());
        assert!(store.get_session(s2.id).await.unwrap().is_some());
        store.delete_agent_sessions(agent_id).await.unwrap();
        assert!(store.get_session(s1.id).await.unwrap().is_none());
        assert!(store.get_session(s2.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_canonical_load_creates_empty() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let canonical = store.load_canonical(agent_id).await.unwrap();
        assert_eq!(canonical.agent_id, agent_id);
        assert!(canonical.messages.is_empty());
        assert!(canonical.compacted_summary.is_none());
        assert_eq!(canonical.compaction_cursor, 0);
    }

    #[tokio::test]
    async fn test_canonical_append_and_load() {
        let store = setup().await;
        let agent_id = AgentId::new();

        let msgs1 = vec![
            Message::user("Hello from Telegram"),
            Message::assistant("Hi! I'm your agent."),
        ];
        store
            .append_canonical(agent_id, &msgs1, None)
            .await
            .unwrap();

        let msgs2 = vec![
            Message::user("Now I'm on Discord"),
            Message::assistant("I remember you from Telegram!"),
        ];
        let canonical = store
            .append_canonical(agent_id, &msgs2, None)
            .await
            .unwrap();

        assert_eq!(canonical.messages.len(), 4);
    }

    #[tokio::test]
    async fn test_rolling_summary_cursor_roundtrip() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let messages: Vec<Message> = (0..6)
            .map(|i| Message::user(format!("turn {i}")))
            .collect();
        store
            .append_canonical(agent_id, &messages, Some(100))
            .await
            .unwrap();

        store
            .update_rolling_summary(agent_id, "summary of first four turns", 4)
            .await
            .unwrap();

        let (summary, cursor, message_count) = store.rolling_context(agent_id).await.unwrap();
        assert_eq!(summary.as_deref(), Some("summary of first four turns"));
        assert_eq!(cursor, 4);
        assert_eq!(message_count, 6);

        store
            .update_rolling_summary(agent_id, "overrun", 99)
            .await
            .unwrap();
        let (_, cursor, message_count) = store.rolling_context(agent_id).await.unwrap();
        assert_eq!(cursor, message_count, "cursor must not overrun messages");
    }

    #[tokio::test]
    async fn test_canonical_context_window() {
        let store = setup().await;
        let agent_id = AgentId::new();

        let msgs: Vec<Message> = (0..10)
            .map(|i| Message::user(format!("Message {i}")))
            .collect();
        store.append_canonical(agent_id, &msgs, None).await.unwrap();

        let (summary, recent) = store.canonical_context(agent_id, Some(3)).await.unwrap();
        assert_eq!(recent.len(), 3);
        assert!(summary.is_none());
    }

    #[tokio::test]
    async fn test_canonical_compaction() {
        let store = setup().await;
        let agent_id = AgentId::new();

        let msgs: Vec<Message> = (0..120)
            .map(|i| Message::user(format!("Message number {i} with some content")))
            .collect();
        let canonical = store
            .append_canonical(agent_id, &msgs, Some(100))
            .await
            .unwrap();

        assert!(canonical.messages.len() <= 60);
        assert!(canonical.compacted_summary.is_some());
    }

    #[tokio::test]
    async fn test_jsonl_mirror_write() {
        let store = setup().await;
        let agent_id = AgentId::new();
        let mut session = store.create_session(agent_id).await.unwrap();
        session.messages.push(Message::user("Hello"));
        session.messages.push(Message::assistant("Hi there!"));
        store.save_session(&session).await.unwrap();

        let dir = tempfile::TempDir::new().unwrap();
        let sessions_dir = dir.path().join("sessions");
        store
            .write_jsonl_mirror(session.clone(), sessions_dir.clone())
            .await
            .unwrap();

        let jsonl_path = sessions_dir.join(format!("{}.jsonl", session.id.0));
        assert!(jsonl_path.exists());

        let content = std::fs::read_to_string(&jsonl_path).unwrap();
        let lines: Vec<&str> = content.trim().split('\n').collect();
        assert_eq!(lines.len(), 2);

        let line1: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(line1["role"], "user");
        assert_eq!(line1["content"], "Hello");

        let line2: serde_json::Value = serde_json::from_str(lines[1]).unwrap();
        assert_eq!(line2["role"], "assistant");
        assert_eq!(line2["content"], "Hi there!");
    }
}
