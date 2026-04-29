//! Shared vocabulary for describing memory surfaces.
//!
//! These types define the dimensions. Individual crates own the surface
//! declarations for the implementation they provide.

use serde::{Deserialize, Serialize};

/// Durable storage surface that owns persistence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemorySubstrateKind {
    SurrealDb,
    Filesystem,
    RuntimeCache,
}

/// Query/model shape used by a memory surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryDataModel {
    Document,
    KeyValue,
    Graph,
    Vector,
    FullText,
    ConfigFile,
    MarkdownFile,
}

/// Runtime agreement a memory surface has with agents, operators, or the kernel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryContract {
    Context,
    Tool,
    Authority,
    Operations,
}

/// Intelligence process that transforms, enriches, ranks, or summarizes memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryIntelligence {
    Embedding,
    Ner,
    Classifier,
    Reranker,
    FullTextTokenizer,
    Summarizer,
    None,
}

/// Code-owned declaration for a memory surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct MemorySurfaceSpec {
    pub id: &'static str,
    pub description: &'static str,
    pub storage_tables: &'static [&'static str],
    pub substrates: &'static [MemorySubstrateKind],
    pub data_models: &'static [MemoryDataModel],
    pub contracts: &'static [MemoryContract],
    pub intelligence: &'static [MemoryIntelligence],
}
