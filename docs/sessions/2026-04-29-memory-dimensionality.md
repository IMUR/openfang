The dimensions framework works today as a descriptive model, but it is not yet fully enforced by the implementation.

The framework says every memory element can be understood through four dimensions:

1. Substrate — where the data is durably grounded.
2. Data Model — how the data is shaped, queried, indexed, or related.
3. Contract — what agreement the data has with agents, tools, runtime, operators, or maintenance paths.
4. Intelligence — what process interprets, transforms, ranks, enriches, or summarizes it.

That framing already describes the current system better than the old layer or three-axis model. It explains why `memories` can be Document + Vector + Full-Text at the same time, why `agent.toml` can be Filesystem authority with a SurrealDB cache, and why `kv` can carry different contracts depending on the key.

But for the framework to become implementation-clean, not just documentation-clean, several refactors would help.

1. KV needs contract separation.

The `kv` table currently mixes several different kinds of data:

- agent tool memory written through `memory_store`
- private profile facts like `self.user_name`
- shared coordination keys
- migration markers like `__openfang_schedules_migrated_v1`
- delivery state such as `delivery.last_channel`
- health and dashboard metric state

Under the dimensions framework, all of those share the same substrate and data model:

- Substrate: SurrealDB
- Data Model: Key-value

But they do not share the same contract.

Some keys have a Tool contract. Some keys have an Authority contract. Some keys have an Operations contract. Some are profile/context support.

That means `kv` cannot honestly be described table-wide as “explicit tool memory.” It is a key-value substrate that hosts multiple contracts.

The implementation could be improved in one of two ways:

- split operational/runtime/profile keys into separate stores or tables, or
- keep one `kv` table but formalize key namespaces and contract ownership.

For example:

- `self.*` = private tool/profile memory
- `shared.*` = shared tool/coordination memory
- `ops.*` = maintenance and migration state
- `runtime.*` = runtime-owned state
- `profile.*` = user/profile authority

The important thing is that the contract should be explicit, not inferred from scattered call sites.

2. KV tools should be renamed or clarified.

The current `memory_recall` tool is misleading. It sounds like semantic recall, but it is actually exact-key KV lookup.

That creates conceptual friction:

- automatic semantic recall searches the `memories` table using vector/full-text retrieval
- explicit `memory_recall` reads a single key from the `kv` table

Those are completely different data models and contracts.

A cleaner tool vocabulary would be:

- `memory_set` / `memory_get` / `memory_list` / `memory_delete` for KV
- `memory_search` or `semantic_recall` for semantic memory search, if exposed to agents

This is not required for the framework to be documented, but it would make the framework much easier for agents and maintainers to understand.

3. Graph context should either become more real or be described more narrowly.

The knowledge graph currently exists as:

- `entities` documents
- `relations` native SurrealDB relation records
- NER-created entity records
- metadata back-links from `memories.metadata.entities`

However, automatic recall does not deeply traverse the native graph as the primary recall path. It mostly uses semantic memory recall, reranking, and entity overlap from memory metadata.

So the graph has multiple contracts:

- Context contract, because it can influence recall ordering
- Tool contract, because tools like `knowledge_add_entity`, `knowledge_add_relation`, and `knowledge_query` expose explicit interaction
- Operations contract, because graph inspection endpoints expose read-only state

But if the canon says the graph is a strong automatic context substrate, the implementation should eventually use native graph traversal and relation semantics more directly in recall.

Otherwise the docs should describe the current truth carefully:

NER primarily builds graph artifacts and metadata back-links. Automatic recall currently uses graph-derived entity overlap, not full graph reasoning.

4. Bootstrap/profile authority needs a cleaner home.

`USER.md` is mostly prompt context. It is read into the prompt as user context.

But `USER.md` also participates in bootstrap suppression: if it has a populated name line, the first-run bootstrap block is suppressed.

That gives `USER.md` two contracts:

- Context contract
- Narrow Authority contract for bootstrap/profile completion

This is acceptable under the dimensions framework, but it should be intentional.

If we want cleaner implementation boundaries, bootstrap completion should probably live in explicit profile/runtime state, such as:

- a profile KV key
- a dedicated profile table
- a manifest/runtime field
- a bootstrap-state record

Then `USER.md` could return to being pure prompt context.

If we keep the current behavior, the docs should explicitly say:

`USER.md` is context with narrow bootstrap/profile authority. It is not general runtime configuration.

5. Semantic memory writes should preserve classification independently of embedding.

The semantic memory write path is conceptually:

- classify the interaction
- embed the interaction
- write the memory record
- index vector/full-text fields
- optionally enrich graph metadata

But if embedding fails, fallback writes can lose some of the richer classification metadata that was already computed.

That weakens the intelligence dimension because classification becomes partly coupled to embedding success.

A cleaner invariant would be:

Classification metadata should be written whenever classification succeeds, regardless of whether embedding succeeds.

Embedding failure should remove the Vector data model for that record, but it should not erase the Classifier intelligence result.

6. Operations contracts should be first-class.

The framework currently has obvious agent-facing contracts:

- Context
- Tool
- Authority

But the implementation also has operator/system-facing contracts:

- health checks
- inspection endpoints
- memory backfill
- migrations
- consolidation
- usage reporting
- read-only graph/semantic inspection

These are not just incidental. They are real ways the system interacts with memory.

So the Contract dimension should include Operations as a first-class contract.

That prevents inspection, backfill, health, and migration paths from being treated as weird exceptions to Context/Tool/Authority.

7. A central dimension registry would make the framework enforceable.

Right now the dimensions model lives in documentation. The code does not have a central representation of:

- which substrate a memory surface belongs to
- which data model it uses
- which contracts apply
- which intelligence paths touch it

That is okay for a documentation-first pass.

But if we want the framework to guide implementation, we could add a lightweight registry or test fixture that declares the memory surfaces and their dimensional placements.

For example:

- `memories`: SurrealDB + Document/Vector/Full-Text + Context/Operations + Embedding/Classifier/Reranker
- `kv:self.*`: SurrealDB + Key-Value + Tool/Profile + None
- `kv:ops.*`: SurrealDB + Key-Value + Operations/Authority + None
- `entities/relations`: SurrealDB + Graph/Document + Context/Tool/Operations + NER
- `agent.toml`: Filesystem + Config + Authority + None
- `USER.md`: Filesystem + Markdown + Context/Profile Authority + None

This registry would not need to drive runtime behavior at first. It could simply power tests and documentation checks so future changes do not drift.

Summary:

The dimensions framework does not require a large refactor to be useful. It already describes the current system better than the old framing.

But the implementation has several places where the framework exposes ambiguity:

- `kv` mixes contracts per key
- `memory_recall` is named like semantic recall but performs exact-key KV lookup
- graph context is only partially integrated into automatic recall
- `USER.md` has both context and bootstrap/profile authority
- classification is too coupled to embedding success in fallback paths
- operations contracts are real but