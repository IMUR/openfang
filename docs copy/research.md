# Research

_External concepts and papers that inform the design of Mind._

---

## Cognitive Science

The architecture draws from established cognitive science models, adapted for LLM constraints.

### Memory Systems Taxonomy

- **Episodic memory** → Trace Store. Personal experiences tagged with temporal and contextual information.
- **Semantic memory** → Schema Store. Decontextualized facts available for fast recall without reconstructing the original experience.
- **Working memory** → Context Buffer. Active, limited-capacity scratchpad for current processing. In LLMs, this is the context window.
- **Procedural memory** → Not directly modeled. LLMs handle this through in-context learning and training.

### Memory Processes

- **Encoding** → Ingestion pipeline. Transforming raw experience into storable representations.
- **Consolidation** → Consolidator. Background reorganization that promotes episodic patterns to semantic memory. In humans, this happens during sleep.
- **Retrieval** → Orchestrator + stores. Cue-dependent, reconstructive, fallible. Not a file lookup.
- **Reconsolidation** → Reconsolidator. Memories become labile when retrieved and can be updated before re-storage.
- **Decay** → Salience Tagger. Unused memories gradually deprioritize, following patterns similar to the Ebbinghaus forgetting curve.

### The Assertion/Inquiry Model

Derived from introspection rather than textbook psychology. The mind operates in confident assertion mode by default, switching to inquiry only when pragmatically triggered. This maps to:

- **Dual-process theory** (Kahneman) — System 1 (fast, automatic) ≈ assertion; System 2 (slow, deliberate) ≈ inquiry
- **Epistemic vigilance** (Sperber) — selective scrutiny applied to incoming information based on source and context

---

## DeepSeek Engram

**Paper:** "Conditional Memory via Scalable Lookup: A New Axis of Sparsity for Large Language Models" (2026)

### Key Concept

Not all knowledge requires dynamic computation. Many multi-token patterns (proper nouns, common phrases) are static — they mean the same thing every time. Transformers waste compute reconstructing these from scratch at every layer.

### Mechanism

- Hash N-grams (bigrams + trigrams) into a large lookup table
- Retrieve pre-learned vector representations in O(1)
- Contextualized gating decides if the retrieved pattern is relevant
- Fuse result back into the main hidden state

### What We Take From It

The _principle_ of separating static knowledge (fast lookup) from dynamic reasoning (compute-intensive). In Mind, this becomes the Schema Store — crystallized patterns that don't need to be reconstructed from episodes every time they're needed.

### What We Don't Take

The specific implementation (hash tables inside transformer layers). Mind operates at the retrieval/orchestration layer, not inside the model architecture.

---

## Anthropic Sleep-time Compute / Auto Dream

**Paper:** "Sleep-time Compute: Beyond Inference Scaling at Test-time" (arxiv:2504.13171)

### Key Concept

Idle time can be used productively. Instead of only computing during active inference, the system can pre-compute useful results based on anticipated needs.

### Two Phases

**Consolidation (Auto Dream):**

- Resolve contradictions in stored knowledge
- Convert relative references to absolute
- Prune stale or superseded facts
- Merge duplicate entries
- Rebuild indexes

**Anticipation (Sleep-time Compute):**

- Analyze past query patterns to predict likely future queries
- Pre-compute embeddings or partial answers
- Amortize expensive computation across related queries

### What We Take From It

The Consolidator subsystem is directly inspired by this. Background processing during idle time that reorganizes knowledge, promotes patterns, and resolves contradictions. The immutable track ensures that consolidation is auditable — you can always see what was changed and why.

---

## XTDB (Architectural Influence)

Not used as a dependency, but its design principles inform the Trace Store.

### Relevant Primitives

- **Append-only transaction log** — immutability as a feature. Aligns with the principle that epistemic history should never be erased.
- **Bitemporal indexing** — tracking "when was this true" separately from "when did the system learn it." This is native epistemological infrastructure.
- **Compactor** — background reorganization without changing content. Direct parallel to systems consolidation in cognitive science.

---

## OpenViking (Architectural Influence)

Not used as a dependency, but its tiered retrieval model informs the Context Buffer.

### Relevant Primitives

- **L0/L1/L2 tiers** — progressive loading from abstract (always present) to detailed (on demand). Maps to how semantic memory is always accessible but episodic detail requires effort.
- **Hierarchical retrieval** — search narrows progressively from broad to specific. Parallels spreading activation in cue-dependent recall.
- **Memory self-iteration** — the system updates its own storage based on usage. Direct parallel to reconsolidation.

---

## Prior Work (Learning From Omnibus)

An earlier project attempted to build a personal knowledge system using literary naming conventions (Archives, Ledger, Biography, Concordance, etc.) wrapping a standard RAG pipeline. The experience revealed:

1. **Renaming infrastructure doesn't create new architecture.** Calling a vector store "Index" and a document store "Biography" adds an abstraction layer without adding capability.
2. **Metaphorical names create false expectations.** Names imply novel concepts; the implementation delivers conventional RAG.
3. **The interesting work is in orchestration, not storage.** What makes a cognitive architecture different from a database is how retrieval, questioning, and reconciliation are coordinated — not what the tables are called.
4. **Cognitive concepts are orthogonal to the pipeline.** Consolidation, inquiry, and salience cut across ingestion/storage/retrieval rather than fitting into pipeline stages.

These lessons directly shaped Mind's principle that technical components keep their own names and cognitive concepts exist as orchestration behaviors.

---

## SurrealKV (Architectural Influence)

Not used as a dependency (BSL 1.1 licensing on SurrealDB; SurrealKV itself is Apache-2.0 but tightly coupled). Studied for storage engine internals relevant to the Trace Store and versioned memory.

**Detailed report:** `docs/research-surrealkv.md`

### Architecture Summary

SurrealKV is an LSM-tree-based, versioned, embedded KV store in pure Rust. It evolved from a VART design that required the entire index in memory. Key inspirations: CockroachDB Pebble (commit pipeline), RocksDB (leveled compaction), WiscKey (value separation), SQLite (B+tree overflow pages).

### What We Take From It

- **Lock-free commit pipeline** — Serialized WAL writes with concurrent memtable applies and atomic visibility publish. Relevant to the Trace Store's append path: how to batch writes without blocking readers.
- **Score-based leveled compaction** — Background reorganization with snapshot awareness. Directly analogous to the Consolidator: reorganizing the immutable log without losing history.
- **Atomic manifest updates** — Changeset + rollback for crash-safe metadata. The Trace Store needs this: if consolidation crashes mid-way, the epistemic history must remain intact.
- **Write stall backpressure** — When compaction falls behind, stall new writes. Prevents unbounded growth during heavy ingestion.
- **Separator-key index gaps** — Upper-bound keys in SSTable index (not exact last keys) with gap-aware seeking. A subtle but important correctness pattern for any sorted structure.
- **CRC32 with masking** — Block checksums use rotated + delta-masked CRC32 to detect common corruption patterns. Simple, proven, worth copying.

### What We Don't Take

- **The full LSM architecture** — Mind's storage layer is `redb` (a B-tree KV store), not an LSM. We study SurrealKV for patterns within components, not for the overall structure.
- **MVCC with sequence numbers** — Mind's versioning is epistemic (belief revision), not transactional isolation. The mechanisms are different even if the surface similarity is tempting.
- **Value Log (WiscKey pattern)** — Value separation optimizes for large values in a document store. Mind's values are structured memory entries, not arbitrary blobs.
- **B+tree versioned index** — An optional secondary index in SurrealKV for timestamp-ordered queries. Mind's temporal needs are different (bitemporal: when true vs. when learned).

### Open Questions

- Does the lock-free commit pipeline pattern apply when writes carry semantic weight (belief revisions) rather than being opaque KV operations?
- Can score-based compaction be adapted to "salience-weighted consolidation" — promoting high-salience patterns more aggressively?
- Is the separator-key gap pattern relevant if we use `redb`'s B-tree directly rather than an LSM?
