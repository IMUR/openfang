# Hybrid Memory System Exploration (SurrealDB + OpenFang)

## Context

This document captures a structured exploration of designing a **tiered, hybrid memory system** using:

- SurrealDB (multi-model backend)
- OpenFang (agent/runtime layer)
- Additional ML components (embeddings, NER, reranking, classification)

Date: 2026-04-01

---

## Core Goal

Design a memory system that is:

- More than retrieval
- Composable + extensible
- Capable of emergent behavior (greater than sum of parts)

---

## Mental Model (Finalized)

Layers:

[ Agent / Orchestrator ]  
        ↓  
[ Memory Interface Layer ]  
        ↓  
[ SurrealDB (Multi-Model Storage + ML Execution) ]

---

## Available ML Capabilities

### 1. Embeddings (Dense Vectors)

- Semantic similarity search
- Retrieval backbone

### 2. Re-ranking

- Refines retrieval results
- Improves ordering

### 3. NER (Named Entity Recognition)

- Extracts structured entities
- Enables graph + filtering

### 4. Classification

- Assigns labels (priority, type, category)
- Can run at ingestion or query time

---

## Key Insight

These are composable signals applied to the same memory object.

---

## Hybrid Memory Record

Each memory entry should support:

- raw_text  
- embedding  
- entities (NER)  
- labels (classification)  
- timestamps  
- relationships

---

## Responsibilities

### SurrealDB Handles:

- Storage (multi-model)
- Vector search
- Query execution
- Model inference (if imported)

### You Handle:

- When to store/retrieve/update
- Which method to use
- How to combine results

---

## OpenViking Insight

### Key Concepts

- Tiered memory (L0 / L1 / L2)
- Hierarchical organization
- Progressive retrieval

---

## Translation to Your System

Dimensions:

- Detail → summaries
- Meaning → embeddings
- Structure → NER
- Priority → classification
- Time → timestamps
- Relationships → graph

---

## Practical Strategy

### Step 1 — Schema

Unified multi-facet memory record

### Step 2 — Ingestion

- generate embedding
- extract entities
- classify (optional)
- attach metadata

### Step 3 — Retrieval

- semantic search
- keyword search
- filtered queries
- structured queries

### Step 4 — Hybrid Query

Combine:

- vector similarity
- text relevance
- classification score

---

## Memory Automation Patterns

1. Event-based (task completion)
2. Feedback-based (user directives)
3. Periodic (session summaries)

---

## Common Failure Mode

Memory not storing automatically → missing triggers

Fix:

- event hooks
- semantic triggers
- post-task commits

---

## Hybrid Search Clarification

Hybrid = Dense vectors + full-text search

---

## Final Takeaways

1. Your model stack is strong:
  - embeddings
  - NER
  - reranking
  - classification
2. Value comes from combining them
3. You are building a:
  Memory fabric, not just a database

---

## End