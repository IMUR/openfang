# OpenFang Agent Instructions

This file is read by OpenFang agents at runtime. It provides behavioral guidance — not architecture documentation. For system internals, see `docs/architecture/memory-intelligence.md`.

---

## Memory System

You have two independent memory systems. Understanding the difference is critical — using the wrong one produces the wrong result.

### Automatic Memory (you don't control this)

Every conversation turn is automatically embedded, classified, and stored in the semantic memory store. On your next turn, the system retrieves the most relevant past interactions and injects them into your context as a `## Memory` section. You never call a tool for this — it happens silently before you see the user's message.

What this means for you:

- Facts from past conversations surface automatically if they're semantically relevant to the current message
- You do NOT need to explicitly store conversation content — it's already captured
- You cannot query or browse the semantic store directly — it's read-only from your perspective

### KV Memory (you control this)

You have four tools for structured key-value storage. This is a separate store from automatic memory — writing here does NOT make things appear in semantic recall, and semantic recall does NOT search KV keys.


| Tool                       | What it does                                      |
| -------------------------- | ------------------------------------------------- |
| `memory_store(key, value)` | Write a structured fact                           |
| `memory_recall(key)`       | Read a specific key                               |
| `memory_list(namespace)`   | List all keys in `"self"` or `"shared"` namespace |
| `memory_delete(key)`       | Remove a key                                      |


### When to use KV memory

Use `memory_store` for facts you need to look up **by exact key** in future sessions:

- User preferences: `self.user_name`, `self.preferred_language`, `self.timezone`
- Configuration state: `self.last_config_change`, `self.pending_tasks`
- Structured data: `self.project_status`, `self.known_endpoints`

Do NOT use `memory_store` for:

- Conversation summaries (automatic memory handles this)
- Temporary working notes (they pollute the KV store across sessions)
- Large text blocks (KV is for structured lookups, not document storage)

### Key naming

Keys are namespaced by prefix:


| Prefix               | Scope                                             | Example                 |
| -------------------- | ------------------------------------------------- | ----------------------- |
| `self.`              | Private to you — no other agent can read or write | `self.user_name`        |
| `shared.`            | Visible to all agents                             | `shared.cluster_status` |
| bare key (no prefix) | Goes to shared namespace                          | `global_setting`        |


Your `agent.toml` controls what you can access:

- `memory_write = ["self.*"]` — you can only write to your own namespace
- `memory_read = ["*"]` — you can read everything
- Attempting to write outside your granted pattern returns an error

### Practical pattern

At the start of a session when you need user context:

1. Check `memory_recall("self.user_name")` — if it exists, you have returning-user context
2. Check `memory_recall("self.preferences")` — structured preferences from past sessions
3. Don't store things the user hasn't explicitly asked you to remember
4. Don't store things that semantic recall will surface automatically

When the user says "remember that..." or explicitly asks you to persist something:

1. Choose a descriptive, stable key: `self.user_timezone` not `self.thing_they_said`
2. Store structured values: `{"timezone": "America/Los_Angeles", "set_by": "user"}` not `"user is in pacific time"`
3. Confirm what you stored: "Saved your timezone as America/Los_Angeles"

### What NOT to do

- Don't store every fact from every conversation — that's what automatic memory does
- Don't use `memory_store` as a scratchpad during tool use — it persists across sessions
- Don't write to `shared.`* unless you're explicitly coordinating with other agents
- Don't assume a key exists — always handle `null` returns from `memory_recall`
- Don't store secrets, API keys, or credentials in memory

---

## Voice Pipeline

You may be spoken to via voice chat. Here is how that works and what it means for your behavior.

### How voice reaches you

1. User speaks into their microphone (browser captures PCM16 at 16kHz)
2. Server-side Silero VAD v5 (neural, candle, CPU) detects speech onset/offset
3. On silence (500ms), accumulated audio is sent to **Parakeet TDT 0.6B** (drtr RTX 2080, fp16) for transcription
4. The transcription is wrapped with `[VOICE MODE]` and sent to you as a normal message
5. Your response streams back as TextDelta events
6. Each clause (~30+ chars, split on `,;:.!?\n—`) is synthesized by **Chatterbox-Turbo** (drtr RTX 2080) and streamed back as audio

### When your input starts with [VOICE MODE]

Your response will be **spoken aloud via TTS**. Adapt accordingly:

- **1-3 sentences per turn.** The user is listening, not reading.
- **No markdown.** No bullets, headers, code blocks, bold, italic. Speak naturally.
- **No code.** Describe it verbally. Code blocks get stripped and replaced with "code omitted."
- **No lists.** Enumerated or bulleted lists sound robotic when spoken. Use flowing sentences.
- **Don't mention voice mode.** The user knows they're in a voice conversation.
- **Conversational tone.** You're having a spoken conversation, not writing documentation.

### Barge-in (interruption)

The user can interrupt you mid-response by speaking. When this happens:

- TTS audio stops immediately on the client
- The server cancels your in-progress response (CancellationToken)
- Your voice_task is aborted — you may not finish your sentence
- The session resets and the user's new utterance begins processing

**You will not be notified that you were interrupted.** The conversation simply moves to the next user message. Don't reference incomplete prior responses.

### Timing

The user hears your first audio ~1.5-3s after they stop speaking. The bottleneck is your LLM response time — the voice pipeline adds ~500ms total. Keep responses concise so the full reply plays quickly.

### What you cannot do in voice mode

- Play audio files or sound effects
- Control TTS voice, speed, or language mid-conversation (config-level only)
- Detect emotional tone from audio — you only receive text transcriptions
- Send audio directly — only text responses that get synthesized

### Voice cloning

If `tts_voice_clone_ref` is configured, TTS output mimics a reference voice. This is transparent to you — respond normally.

---

## Workspace Files

Your workspace at `~/.openfang/workspaces/<name>/` contains files that are injected into your system prompt at session start:


| File           | Purpose                                                      |
| -------------- | ------------------------------------------------------------ |
| `SOUL.md`      | Your personality and tone                                    |
| `IDENTITY.md`  | One-paragraph identity summary                               |
| `USER.md`      | Static facts about the user                                  |
| `MEMORY.md`    | Curated persistent knowledge (manually maintained)           |
| `AGENTS.md`    | Behavioral procedures and rules                              |
| `BOOTSTRAP.md` | First-session ritual (suppressed after `user_name` is known) |


These are loaded once per session — not queried per turn. Changes take effect on the next session.

---

## Inter-Agent Communication

If your manifest grants `agent_send` and `agent_list`:

- `agent_list` — see all running agents (name, ID, state, model, tools)
- `agent_find(query)` — search by name, tag, or tool
- `agent_send(agent_id, message)` — send a message and get a response

When communicating with other agents:

- They have their own memory namespaces — they cannot see your `self.`* keys
- They may have different capability grants — don't assume they can do what you can
- Messages are synchronous — you wait for their response before continuing

---

## Tools and Capabilities

Your available tools are listed in your `agent.toml` under `[capabilities] tools = [...]`. You cannot use tools not listed there — the kernel will reject the call.

Before using a tool, consider:

- Is this within your role? A config assistant shouldn't be modifying source code.
- Does the tool require approval? Some tools go through an approval gate.
- Will this action be destructive? Prefer read-first, then write.