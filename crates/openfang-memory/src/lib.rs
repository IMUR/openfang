//! Memory substrate for the OpenFang Agent Operating System.
//!
//! Provides a unified memory API backed by embedded SurrealDB:
//! - **Structured store**: Key-value pairs, agent state
//! - **Semantic store**: Text + vector search
//! - **Knowledge graph**: Entities and relations (SurrealDB graph edges)
//! - **Session store**: Conversation history + cross-channel canonical sessions
//! - **Usage store**: Token tracking + cost aggregation
//! - **Consolidation engine**: Background decay + pruning
//!
//! Agents interact with a single `Memory` trait that abstracts over all stores.

pub mod consolidation;
pub mod db;
pub mod knowledge;
pub mod semantic;
pub mod session;
pub mod structured;
pub mod usage;

mod substrate;
pub use substrate::MemorySubstrate;
