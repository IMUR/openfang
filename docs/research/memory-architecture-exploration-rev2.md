---
date: 2026-04-26
type: architecture
topics: [openfang, memory-intelligence, surrealdb, architecture]
---

# Memory Architecture: From Accident to Intent

## The Why

The memory system should feel like invisible magic. An agent in conversation surfaces context it has no business knowing — the user's name, a fact from three weeks ago, a pattern across sessions — and the user has no idea any machinery is involved. The system just *remembers*, the way a thoughtful colleague would.

This works today. But the architecture supporting it has been built reactively, decision by decision, without an organizing model. The result is a system that works as long as nothing pushes too hard against it.

This document is the organizing model. It does not propose new patterns; it codifies the ones OpenFang has already organically discovered.

---

## The Friction

Four problems are entangled.

**1. The "three layer" mental model is arbitrary.** Layer 1, Layer 2, and Layer 3 were named in the order they were built, not because they describe coherent concepts. They mix categories — Layer 1 is described as a behavior, Layer 2 as a tool interface, Layer 3 as a data location. They don't compare cleanly because they aren't the same kind of thing.

Look at the actual prompt assembly in `crates/openfang-runtime/src/prompt_builder.rs`: roughly seventeen numbered sections with decimal insertions (1, 1.5, 2, 2.5, 3, 4, 5, 6, 7, 7.5, 8, 9, 9.1, 9.5, 10, 11, 13, 14, 15) and section 12 relocated into `build_canonical_context_message()` to preserve provider prompt-cache hits across turns. The decimal numbering is a fossil record of organic insertion. The relocation is sophistication forced by provider economics. Neither is captured by "Layer 1, 2, 3."

**2. Authority drifts between SurrealDB and the filesystem.** Workspace files (`agent.toml`, `SOUL.md`, `USER.md`) are authoritative for declarative agent state. But SurrealDB has been silently overriding them on boot — change a file, see no effect, eventually discover that a cached database manifest is winning. The `agent.toml` case was recently fixed by making the disk file fully replace the embedded DB manifest at boot. The pattern hasn't been generalized.

**3. The static substrate has two ingestion paths and they aren't named.** Files like `SOUL.md`, `USER.md`, and `MEMORY.md` load through `PromptContext` into the persona section of the system prompt. Files like `AGENTS.md`, `VOICE.md`, and `context.md` load through `workspace_context.rs` into a workspace summary. The doc canon treats these as one path; the code treats them as two. One defines *who the agent is*; the other defines *what situation the agent is in*. Conflating them obscures both.

**4. No connective tissue across data models.** SurrealDB hosts five different kinds of data — documents, key-value, graph, vector, full-text — in one substrate, but the application treats each as an island. Every connection between a memory record, its extracted entities, and its KV facts has to be hand-coded. Multi-model in storage; single-model in awareness.

---

## What's Actually There

OpenFang's memory infrastructure has three orthogonal dimensions.

### Dimension 1 — Data Models (SurrealDB)

Five data models share one unified database at `~/.openfang/data/openfang.db/`:

| Data Model    | What It Stores                | Primary Access      |
|---------------|-------------------------------|---------------------|
| Document      | Memory records with metadata  | Automatic recall    |
| Key-value     | Exact-key facts               | Explicit tools      |
| Graph         | Entities and relationships    | Recall scoring      |
| Vector        | Mathematical embeddings       | Automatic recall    |
| Full-text     | Word-indexed content          | Automatic recall    |

### Dimension 2 — Static Substrate (Workspace Files)

Workspace files break into three roles by ingestion path:

| Role          | Files                                                              | Loaded Into                       | Cadence                             |
|---------------|--------------------------------------------------------------------|-----------------------------------|-------------------------------------|
| Persona       | `SOUL.md`, `IDENTITY.md`, `USER.md`, `MEMORY.md`, `BOOTSTRAP.md`   | `PromptContext` persona section   | Session start                       |
| Context       | `AGENTS.md`, `VOICE.md`, `TOOLS.md`, `HEARTBEAT.md`, `context.md`  | Workspace context summary         | Session start; `context.md` per-turn |
| Configuration | `agent.toml`, `config.toml`                                        | Kernel + agent runtime            | Boot                                 |

Each prompt section has an explicit byte cap (500–8000 bytes). The **file-read cap** (32 KB, in `workspace_context.rs`) and the **prompt-injection cap** (typically 8 KB, in `prompt_builder.rs`) are distinct: a file larger than the read cap is skipped; a file larger than the injection cap is truncated.

### Dimension 3 — Intelligence Models (Candle, CPU)

Four BERT-family models transform memory inline:

| Model      | Writes To                                  | Read During              |
|------------|--------------------------------------------|--------------------------|
| Embedding  | Vector index                               | Automatic recall         |
| NER        | Graph (entities + relations)               | Graph-boosted scoring    |
| Classifier | Document metadata (`scope`, `category`)    | Scope-weighted ordering  |
| Reranker   | (read-only)                                | Recall re-ranking        |

The KV store has no intelligence model on top of it; agents write to it directly. The full-text index uses a snowball English tokenizer, not a Candle model. Every other dynamic data model is fed by exactly one intelligence model.

---

## The Reframing

Drop "Layer 1, 2, 3." The system actually has three orthogonal axes, and any piece of memory infrastructure has a position on each.

**Axis A — Storage substrate (where data lives):** *Dynamic* (SurrealDB) or *Static* (workspace files).

**Axis B — Access pattern (how data reaches the prompt):** *Automatic* (pulled in without the agent asking), *Explicit* (pulled by agent tool calls), or *Authoritative* (read at boot, defines runtime behavior).

**Axis C — Intelligence backend (what transforms the data):** *Embedding*, *NER*, *Classifier*, *Reranker*, or *None* (no model in the loop).

The whole memory system fits on a grid:

| Subsystem                   | Substrate | Access        | Intelligence Backend                     |
|-----------------------------|-----------|---------------|------------------------------------------|
| Semantic store (`memories`) | Dynamic   | Automatic     | Embedding + Classifier + Reranker        |
| Knowledge graph             | Dynamic   | Automatic     | NER                                      |
| KV store                    | Dynamic   | Explicit      | None                                     |
| Full-text index             | Dynamic   | Automatic     | None (tokenizer only)                    |
| Persona files               | Static    | Automatic     | None                                     |
| Context files               | Static    | Automatic     | None                                     |
| Configuration files         | Static    | Authoritative | None                                     |

Every feature in the memory system has a row. New features should be placed on the grid before they're built — if a feature doesn't have a clean position, the architecture isn't ready for it.

---

## Worked Example: Bootstrap Suppression

`prompt_builder.rs` decides whether to include the first-run bootstrap protocol by checking three sources:

1. Shared KV `user_name` exists — *dynamic + explicit + no intelligence backend*
2. Recalled memory contains `user_name` — *dynamic + automatic + embedding + classifier + reranker*
3. `USER.md` has a populated Name line — *static + automatic + no intelligence backend*

The code comment notes: "USER.md is human-curated prompt context, not a control plane." But the same function is using it as a control-plane signal. The three-axis vocabulary makes the tension visible: bootstrap suppression is a single rule reading three sources spanning both substrates, two access patterns, and the full intelligence stack on one of them.

Either the rule needs to formalize that breadth, or `USER.md` needs a clean control-plane sibling. This is the canonical "organic rule that needs naming." There are others.

---

## Three Architectural Priorities

### 1. Drop the historical layering. Use the three axes.

Stop calling memory subsystems Layer 1, 2, 3. Adopt the substrate / access / intelligence framing above. Update the canon docs (`/mnt/ops/canon/openfang/memory-intelligence.md`) to describe each subsystem by its position on each axis. The seventeen-section prompt assembly in `prompt_builder.rs` is the existing implementation — document what it actually does, in the vocabulary it already implies.

### 2. Codify authority with a clear lock boundary.

Workspace files are authoritative. SurrealDB caches what's needed at runtime but never overrides the disk source. The `agent.toml` enforcement now needs to extend to every workspace file by convention.

The complementary invariant is the **lock boundary**:

> The dynamic substrate is API-mediated. Only the static substrate is filesystem-writable.

The daemon holds the exclusive SurrealDB lock. External writers (cron jobs, integrations, sidecar agents) cannot touch the database directly — they go through the daemon API. This is why semantic memory verification yesterday had to fall back to non-locking WAL string inspection: there was no read-only API path. That gap should close.

The patterns for **bounded agent influence on authoritative files** already exist:

- **HTML-comment authorship declaration** (file-level). `USER.md` carries `<!-- Updated by the agent as it learns about the user -->`; `MEMORY.md` carries `<!-- Curated knowledge the agent preserves across sessions -->`. Files without a comment are static and human-only. Standardize this across all eight corefile templates.

- **`context.md` + `cache_context` manifest flag** (per-turn refresh). Implemented in `crates/openfang-runtime/src/agent_context.rs`, tracked by issue #843. Bounded, graceful on read failure, opt-out per agent. This is the canonical mechanism for "external writers update agent prompt context." Future "live data injected by external process" use cases funnel here.

- **`.openfang/` directory separation** (path signals authority). Human-authored files in workspace root; system-managed sidecar state under `.openfang/`. Already enforced.

Section-level authorship within a single file (delimited regions where the agent updates a specific block while leaving the rest untouched) is a natural future refinement, but it requires a parser that respects regions and is a meaningful jump beyond the file-level convention. File-level lands first.

### 3. Introduce temporal markers as a passive cross-model index.

Every record in every data model carries a native `datetime` field — not an RFC3339 string. Time becomes the implicit connector across documents, KV, graph, and vector data. Records from different models created within a time window become queryable as a coherent slice without explicit cross-model wiring.

The migration is bounded but not free: SurrealDB can convert RFC3339 strings via `time::from::rfc3339()`, and `SCHEMALESS` tables let old and new records coexist on storage. The catch is on the Rust side — serde fields typed as `String` will fail when the DB returns a `datetime`. Plan a typed cutover, not a flag flip.

---

## Where the Boundary Is

This reframing applies to the memory and context system: the SurrealDB substrate, the workspace files, the four Candle intelligence backends, and the prompt builder that joins them. It is silent on:

- **Channel adapters** (`telegram`, `discord`, `slack`, etc.) — they have their own context-shaping logic per channel, applied after the prompt is built.
- **MCP server integration** — has its own allowlist and tool-summary authority, currently with a known parser bug for hyphenated server names.
- **Capability enforcement** — `MemoryRead` / `MemoryWrite` glob checks live in the kernel and resolve before any of these axes apply.
- **LLM provider routing** — has the same structural authority chain (`provider_defaults()` → `provider_urls`/`provider_api_keys` → `secrets.env`). The vocabulary travels; the application doesn't.

These systems intersect with memory but don't share its authority model. They get their own architectural docs.

---

## The Spirit

The system should be teachable in one paragraph. Today it requires a 500-line architecture document and a senior dev agent to navigate. The reframing is not about changing what the system does — it is about making *why* it does what it does possible to hold in your head.

The patterns are already in the code. The work is naming them, documenting them, and standardizing them so the next dev agent who looks at OpenFang doesn't have to reverse-engineer the architecture from breaking.

If a new dev agent can read this document and immediately know where a feature should live, the reframing has succeeded.