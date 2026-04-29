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

Three problems are entangled.

**1. The "three layer" mental model is arbitrary.** Layer 1, Layer 2, and Layer 3 were named in the order they were built, not because they describe coherent concepts. They mix categories — Layer 1 is described as a behavior, Layer 2 as a tool interface, Layer 3 as a data location. They don't compare cleanly because they aren't the same kind of thing.

Look at the actual prompt assembly in `crates/openfang-runtime/src/prompt_builder.rs`: fifteen numbered sections (1, 2, 2.5, 3, 4, 5, 6, 7, 7.5, 8, 9, 9.1, 9.5, ...). The decimal numbering is a fossil record of organic insertion — sections wedged between existing sections as needs emerged. The "three layers" framing flattens that detail into something the code doesn't actually match.

**2. Authority drifts between SurrealDB and the filesystem.** Workspace files (`agent.toml`, `SOUL.md`, `USER.md`) are authoritative for declarative agent state. But SurrealDB has been silently overriding them on boot — change a file, see no effect, eventually discover that a cached database manifest is winning. The `agent.toml` case was recently fixed by making the disk file fully replace the embedded DB manifest at boot. The pattern hasn't been generalized; other files have no documented authority story.

**3. No connective tissue across data models.** SurrealDB hosts five different kinds of data — documents, key-value, graph, vector, full-text — in one substrate, but the application treats each as an island. Every connection between a memory record, its extracted entities, and its KV facts has to be hand-coded. Multi-model in storage; single-model in awareness.

---

## What's Actually There

OpenFang's memory infrastructure spans two storage substrates.

**SurrealDB** hosts five data models in one unified database at `~/.openfang/data/openfang.db/`:

| Data Model    | What It Stores                | Example                              |
|---------------|-------------------------------|--------------------------------------|
| Document      | Arbitrary structured records  | Memory records with metadata         |
| Key-value     | Exact-key facts               | `self.user_name = "Alice"`           |
| Graph         | Entities and relationships    | Alice → works_at → Acme              |
| Vector        | Mathematical embeddings       | Semantic similarity search           |
| Full-text     | Word-indexed content          | Keyword search                       |

**Workspace files** are human-authored markdown and TOML scanned at session start. The set is fixed: `AGENTS.md`, `SOUL.md`, `VOICE.md`, `TOOLS.md`, `IDENTITY.md`, `HEARTBEAT.md`, `context.md`, plus the agent's `agent.toml` manifest. Each has a defined slot in the system prompt and an explicit byte cap (500–8000 bytes per section).

These two substrates are joined by the **prompt builder** (`crates/openfang-runtime/src/prompt_builder.rs`), which assembles them into the system prompt on every turn. That assembly is the de-facto memory architecture; the canon docs have been lagging behind it.

---

## The Reframing

Drop the "Layer 1, 2, 3" framing. Replace it with two orthogonal axes already implicit in the code.

**Axis 1 — Storage substrate (where data lives):**

- *Dynamic substrate:* SurrealDB's five data models
- *Static substrate:* Workspace files

**Axis 2 — Access pattern (how data reaches the prompt):**

- *Automatic:* Pulled into context without the agent asking — semantic recall, persona files, workspace context
- *Explicit:* Pulled by agent tool calls — `memory_store`, `memory_recall`
- *Authoritative:* Defines runtime behavior, read at boot — `agent.toml`, `config.toml`

Every piece of memory infrastructure has a position on both axes. The semantic store is *dynamic + automatic*. The KV store is *dynamic + explicit*. `SOUL.md` is *static + automatic*. `agent.toml` is *static + authoritative*. The position determines the contract.

This gives the team a coherent vocabulary for *why* a piece of state lives where it does — instead of "because Layer N."

---

## Three Architectural Priorities

### 1. Drop the historical layering. Use behavior.

Stop calling memory subsystems Layer 1, 2, 3. Adopt the storage / access framing above. Update the canon docs to describe each subsystem by its position on both axes. The fifteen-section prompt assembly in `prompt_builder.rs` is the existing implementation — document what it actually does, in the vocabulary it already implies.

### 2. Codify authority with controlled influence.

Workspace files are authoritative. SurrealDB caches what's needed at runtime but never overrides the disk source. This principle was recently enforced for `agent.toml` and now needs to extend to every workspace file by convention.

The patterns for **bounded agent influence on authoritative files** already exist in OpenFang. The work is not invention — it is **standardization**.

**a) HTML-comment authorship declaration (file-level).** Each core file template in `docs/architecture/corefiles/` carries a top-level comment that names the writer:

- `<!-- Updated by the agent as it learns about the user -->` (`USER.md` — agent-updateable)
- `<!-- Curated knowledge the agent preserves across sessions -->` (`MEMORY.md` — agent-curated)
- `<!-- Visual identity and personality at a glance. Edit these fields freely. -->` (`IDENTITY.md` — human-edited)
- *No comment* = static, human-authored (`SOUL.md`, `AGENTS.md`)

This convention should be made canonical: every workspace file declares its writer in its first comment line. The pattern can be refined later to **section-level** authorship within a single file (delimited regions where the agent updates a specific block while leaving the rest untouched), but the file-level convention is the foundation that needs to land first.

**b) `context.md` + `cache_context` manifest flag (per-turn refresh).** This is OpenFang's existing mechanism for "external writers update the prompt context, agent re-reads it every turn." Implemented in `crates/openfang-runtime/src/agent_context.rs`, tracked by issue #843. It is bounded (32 KB cap), graceful (falls back to last cached content if a writer is mid-rewrite), and configurable per agent (`cache_context = true` opts back into read-once behavior). Every "external influence" use case should funnel through this.

**c) `.openfang/` directory separation (path signals authority).** Human-authored files live in the workspace root. System-managed sidecar state (`workspace-state.json`, the SurrealDB database, model caches) lives under `.openfang/`. The path itself is the authority signal. Already enforced; needs to stay enforced.

### 3. Introduce temporal markers as a passive cross-model index.

Every record in every data model carries a native `datetime` field — not an RFC3339 string. Time becomes the implicit connector across documents, KV, graph, and vector data. Records from different models created within a time window become queryable as a coherent slice without explicit cross-model wiring.

The migration is bounded: SurrealDB can convert RFC3339 strings via `time::from::rfc3339()`, and the schemaless tables let old and new records coexist during cutover. Once datetime is native, time-based consolidation logic (`accessed_at < now - 7d`) moves from Rust into SurrealQL — fewer round trips, more readable queries, and a foundation for downstream features that group memory by temporal proximity.

---

## Where the Boundary Is

This reframing applies to the memory and context system: the SurrealDB substrate, the workspace files, and the prompt builder that joins them. It is silent on:

- **Channel adapters** (`telegram`, `discord`, `slack`, etc.) — they have their own context-shaping logic per channel, applied after the prompt is built.
- **MCP server integration** — has its own allowlist and tool-summary authority, currently with a known parser bug for hyphenated server names.
- **Capability enforcement** — `MemoryRead` / `MemoryWrite` glob checks live in the kernel and resolve before any of these axes apply.
- **LLM provider routing** — selection of which model handles a turn is upstream of prompt assembly.

These systems intersect with memory but don't share its authority model. They get their own architectural docs.

---

## The Spirit

The system should be teachable in one paragraph. Today it requires a 500-line architecture document and a senior dev agent to navigate. The reframing is not about changing what the system does — it is about making *why* it does what it does possible to hold in your head.

The patterns are already in the code. The work is naming them, documenting them, and standardizing them so the next dev agent who looks at OpenFang doesn't have to reverse-engineer the architecture from breaking.

If a new dev agent can read this document and immediately know where a feature should live, the reframing has succeeded.
