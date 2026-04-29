# Memory System Analysis

*April 22, 2026*

---

## Architecture

OpenFang's memory system has two layers:

### Key-Value Memory (runtime)

- **Two namespaces**: `self.`* (agent-private) and shared (cross-agent)
- **Operations**: store, recall, list, delete
- **Behavior**: Keys are simple strings, values are JSON-encoded. Persisted across sessions.
- **Injection**: Recalled memories are pre-loaded into the system prompt before the LLM sees each input.

### SurrealDB Substrate (long-term)

- Vector search via HNSW with BGE-small embeddings
- Named entity recognition, cross-encoder reranking, classification
- Workspace markdown files: `MEMORY.md`, `SOUL.md`, `IDENTITY.md`, `USER.md` — flat files on disk

These two layers feel somewhat disconnected right now. The KV store is what agents actually use in practice; the vector substrate is available but doesn't seem to be wired into the recall path.

---

## How Recall Actually Works

At the start of each turn, the system pre-loads memories and injects them into the prompt. The agent sees something like:

> *Use the recalled memories below to inform your responses. Only call memory_recall if you need information not already shown here.*

This means every turn gets a snapshot of memory injected automatically. The agent can also call `memory_recall` explicitly during a turn for keys not in the pre-loaded set.

There's no visible relevance scoring or semantic matching between the current input and stored memories. It's a flat dump, not a retrieval.

---

## What the Namespaces Actually Contain

### Private (self.*) — 12 keys

A mix of operational notes, debugging observations, and session context:


| Category                | Examples                                                |
| ----------------------- | ------------------------------------------------------- |
| Operational preferences | Lesson about using live API endpoints over config files |
| Debugging notes         | Voice buffer issues, TTS truncation, ghost messages     |
| Design concepts         | Cassette architecture (detailed), prism metaphor        |
| Session tracking        | Voice chat progress, pipeline issue notes               |
| Tests                   | Round-trip test from April 7                            |


### Shared — 14 keys

Cross-agent state and user preferences:


| Category       | Examples                                          |
| -------------- | ------------------------------------------------- |
| User identity  | Name, preferred agent name                        |
| Agent state    | Infisical sync status (last sync, counts, errors) |
| Infrastructure | SMB network drive path                            |
| Schedule state | Empty schedule tracker, migration flag            |


---

## Storage Patterns

### Value formats are inconsistent

- Some values are JSON arrays: `["Bridging human-AI gap...", "North Star as..."]`
- Some are structured objects: `{"Projector": "prtr", ...}`
- Some are free text paragraphs (the cassette architecture entry is a full markdown doc)
- Some are single values: dates, counts, status strings

This isn't necessarily a problem — it's flexible — but it means there's no uniform way to process memories programmatically. Each value needs to be understood on its own terms.

### No visible TTL or expiry

Memories from weeks ago (April 6-7 test keys, resolved debugging sessions) sit alongside current context. There's no mechanism to age out or archive stale entries.

### MEMORY.md is empty

The workspace file exists as a template with a comment header, but no agent has written curated long-term knowledge to it yet. All persistent knowledge lives in the KV store.

---

## Cross-Agent Memory

The shared namespace allows agents to coordinate:

- `infisical-sync-hand` writes sync status that other agents could read
- User identity (name, preferences) is shared
- Infrastructure knowledge (SMB paths) is accessible to all agents

There's no visible conflict resolution. If two agents write to the same key, last-write-wins appears to be the rule.

---

## Observations

1. **Memory is passive, not active.** The system dumps everything into the prompt; the agent decides what's relevant. There's no pre-filtering or ranking.
2. **Recall is key-based, not semantic.** You get a memory by knowing its key name. You can't search by content or meaning. This makes memory useful for things you knew to store but not for surfacing unexpected connections.
3. **The vector substrate is underutilized.** SurrealDB has HNSW, embeddings, NER, reranking — but the actual recall path is just key-value lookups. The semantic retrieval capability exists but doesn't seem wired in.
4. **Agents don't curate their own memory.** MEMORY.md sits empty. The KV store accumulates without pruning. There's no cycle of reflection, consolidation, or archival.
5. **Memory is write-heavy relative to its value.** Agents store observations liberally (debugging notes, test keys, passing thoughts) but rarely revisit them. The signal-to-noise ratio degrades over time.

---

## Live Agent Memory Audit (April 22, 2026)

Tested by sending messages to the **assistant** agent via `agent_send`.

### Tool availability discrepancy


| Agent                    | Tools available                                                 |
| ------------------------ | --------------------------------------------------------------- |
| **openfang-config** (me) | `memory_store`, `memory_recall`, `memory_list`, `memory_delete` |
| **assistant**            | `memory_store`, `memory_recall` only — **no `memory_list`**     |


The assistant cannot enumerate keys. It can only probe by exact key name, which means it's blind to what's actually stored unless someone tells it the key.

### Write permissions by namespace


| Namespace          | assistant      | openfang-config          |
| ------------------ | -------------- | ------------------------ |
| `self.*` (private) | ✅ Read/write   | ✅ Read/write/list/delete |
| shared (no prefix) | ❌ Write denied | ✅ Read/write/list/delete |


The assistant **cannot write to shared memory**. This means:

- It can persist private context across sessions, but no other agent can read it.
- It can't contribute to cross-agent coordination (user identity, sync status, etc.).
- Shared keys that exist (from other agents or manual writes) are invisible to it — `memory_recall` returns nothing.

### What the assistant actually sees in its prompt

The assistant confirmed it receives a "Recalled memories" section injected by the runtime. However, the content of that section is **conversation history**, not KV store data. Specifically, it saw a summary of our previous exchange (truncated mid-sentence) injected as a "Recalled memory."

This means there are **two separate persistence mechanisms** being conflated:

1. **Session/conversation persistence** — runtime auto-saves and re-injects prior turns as "Recalled memories." This works regardless of the KV store.
2. **KV memory store** — explicit key-value pairs via `memory_store`/`memory_recall`. This is what agents use intentionally.

The prompt template presents both under the same "Memory" heading, which is confusing.

### Other agents unreachable


| Agent               | Status      | Reason                 |
| ------------------- | ----------- | ---------------------- |
| General Assistant   | Boot failed | Missing `GROQ_API_KEY` |
| collector-hand      | Crashed     | Unknown                |
| infisical-sync-hand | Crashed     | Unknown                |
| clip-hand           | Crashed     | Unknown                |


Could not audit their memory state.

### Updated observations

1. **Memory permissions are asymmetric across agents.** Not all agents have the same tools or the same write access. The assistant can't write shared memory and can't list keys — making it effectively memory-blind for cross-agent coordination.
2. **Conversation history is masquerading as memory.** The runtime injects prior turns as "Recalled memories," but this is session persistence, not intentional long-term memory. Agents (and operators) may confuse the two.
3. **The "Recalled memories" header is misleading.** It suggests semantic retrieval or curated knowledge, but the actual content is just prior conversation turns. The name should distinguish between auto-persisted context and intentional memory.

*Based on runtime observation, system prompt introspection, and namespace inspection. No source code examined.*