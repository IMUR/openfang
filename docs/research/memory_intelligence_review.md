# Memory Intelligence Plan — Architect Review + Directed Reality Checks

Date: 2026-04-01

---

## Purpose

This document refines the existing **Memory Intelligence Completion Plan** by:

1. Preserving all validated insights from the development agent
2. Adding architectural guidance
3. Introducing **explicit reality-check directives** (ONLY where assumptions may exist)
4. Adding **acceptance criteria and validation gates**

This is intended to be fed back into the development agent.

---

## Ground Rule

If any step below is uncertain:

> The development agent MUST verify directly in the codebase  
> instead of relying on inference.

---

## Section 1 — Confirmed by Development Agent (Trusted)

The following are treated as **ground truth**:

- Knowledge graph exists but is not used in recall
- `populate_knowledge_graph` uses incorrect memory identifiers
- Embeddings, NER, reranker are wired and functioning
- Hybrid search (HNSW + BM25) is implemented
- Scope is hardcoded to `"episodic"`
- Metadata is empty (`HashMap::new()`)
- Classification does not exist
- Consolidation lacks summarization

(Source: development agent plan)

---

## Section 2 — Mandatory Reality Checks

The development agent should verify ONLY these items:

### RC-1 — MemoryId Propagation
- Confirm `remember_with_embedding_async` return value is discarded
- Confirm no downstream usage of real MemoryId in graph population

### RC-2 — Knowledge Graph Link Integrity
- Verify whether any valid memory→entity linkage exists today
- Confirm whether relations currently resolve to real memory records

### RC-3 — Metadata Mutability
- Confirm whether memory records can be updated post-write
- If not: identify required method (e.g. `update_metadata`)

### RC-4 — NER Availability at Recall Time
- Confirm whether NER driver is callable from recall path
- Confirm threading/async constraints

### RC-5 — Consolidation Execution Context
- Confirm where consolidation loop runs (kernel vs substrate)
- Confirm whether LLM access can be injected there

### RC-6 — Hook System Capability
- Verify ability to add:
  - AfterMemoryStore
  - AfterMemoryRecall

---

## Section 3 — Required Additions to Plan

### 3.1 Acceptance Criteria (MANDATORY)

Each phase must define success.

#### Phase 0 (Graph Wiring)
- Graph traversal returns memories linked to entities
- No literal string IDs remain

#### Phase 1 (Metadata + Classification)
- ≥90% of new memories contain:
  - scope
  - metadata fields
- Directive detection produces declarative memories

#### Phase 2 (Graph Recall)
- Graph boost changes top-k recall results in measurable cases
- Entity overlap increases recall relevance

#### Phase 3 (Summarization)
- L1 summaries reduce token footprint
- Summaries remain traceable to source memories

#### Phase 4 (Automation)
- Memory creation triggered by:
  - tools
  - user directives
  - periodic jobs

---

### 3.2 Observability Layer (NEW)

Add instrumentation:

Each recall should log:
- retrieval path used (vector / hybrid / graph)
- graph contribution score
- rerank delta
- scope distribution of results

Each write should log:
- source type
- classification result
- entities extracted

---

### 3.3 Backfill / Reindex Strategy (NEW)

The system must support:

- Re-running NER on existing memories
- Recomputing classification
- Updating metadata
- Re-linking graph edges

Reality check required:
- Can existing records be mutated?
- If not: define migration path

---

### 3.4 Summary Safety (NEW)

Summaries must NOT overwrite truth.

Requirements:
- Always store `summarized_from`
- Never delete original memories prematurely
- Allow re-expansion of summaries into source data

---

## Section 4 — Revised Implementation Order

1. Phase 0 — Fix graph wiring
2. Add observability (NEW requirement)
3. Phase 1 — Metadata + classification
4. Phase 4c — User directive detection (early win)
5. Phase 2 — Graph-aware recall
6. Phase 4b / 4d — Tool + periodic memory
7. Phase 3 — Summarization
8. Phase 5 — ML classification (ONLY if justified)

---

## Section 5 — Classification Gate

DO NOT implement ML classification until:

- Rule-based classification improves recall measurably
- Metadata is fully utilized in retrieval
- Graph + scope signals are proven valuable

---

## Section 6 — Core Architectural Principle

This system is NOT:

- a vector database
- a graph database
- a classification pipeline

It IS:

> A **multi-signal memory fabric**

Each memory contains:
- semantic signal (embedding)
- structural signal (NER / graph)
- categorical signal (classification)
- temporal signal (timestamps)
- relational signal (edges)

The agent orchestrates usage.

---

## Section 7 — Final Instruction to Development Agent

Do NOT reinterpret the plan.

Instead:

1. Validate reality-check items
2. Confirm or correct assumptions
3. Integrate acceptance criteria
4. Add observability hooks
5. Ensure backfill capability exists

Only after validation:
→ proceed with implementation

---

## End
