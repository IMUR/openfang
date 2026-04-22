### Architecture Guide: Implementing AutoDream Memory Consolidation for AI Agents

#### 1\. Executive Overview of Autonomous Memory Consolidation

The "AutoDream" architecture, discovered through the analysis of the version 2.1.88 leaked source maps (kij.m.map), represents a paradigm shift in persistent context management. It is a background maintenance framework designed to transition short-term session observations into durable, long-term project knowledge.Modeled after the human sleep/REM cycle, AutoDream addresses the critical problem of  **Context Entropy** . In high-frequency development environments, AI memory files naturally accumulate stale debugging notes, contradictory instructions, and redundant patterns. Without active maintenance, this "memory decay" degrades agent performance and consumes disproportionate segments of the context window. AutoDream ensures the agent's knowledge remains sharp and organized by periodically replaying session data, promoting high-signal patterns, and pruning noise.

#### 2\. The Multi-Layered Memory Hierarchy

To maximize  **Token Efficiency Optimization** , AutoDream manages a four-layer memory stack. This structure ensures that only the most critical routing information is loaded at startup, while granular details are "pulled" into the context window only as needed.

* **CLAUDE.md (Core Instructions):**  Authoritative rules, standards, and architecture decisions. This is the highest-priority layer and is always loaded.  
* **Automemory (Topic Files):**  Distributed knowledge stored in subject-specific Markdown files (e.g., debugging.md, patterns.md). These are loaded on-demand via the index.  
* **Memory Index (MEMORY.md):**  A 200-line/25KB limited "pointer" file. By strictly enforcing this limit, the system ensures the "Starting Context" remains lean, preserving the maximum possible token budget for the active reasoning and tool-execution portion of the context window.  
* **Session Memory (Raw Transcripts):**  Local JSONL logs containing the exhaustive history of every tool call and message. These are never loaded in full but serve as the raw data source for consolidation.

##### Memory Comparison Matrix

Feature,Short-Term / Session Memory,Long-Term / Consolidated Memory,Access Pattern  
Written By,AI Agent (Automatically),AutoDream Sub-Agent,N/A  
Primary Storage,Local JSONL session logs,Markdown files (.md),N/A  
Persistence,Temporary (Conversation level),Durable (Project level),N/A  
Context Load,Full transcript (limited),Indexed pointers (Index file),Always loaded (Index) / On-demand (Topics)

#### 3\. The Four Phases of Memory Consolidation

The consolidation execution loop is a structured process designed to achieve  **Idempotency**  and semantic clarity across project knowledge.

1. **Phase 1: Orientation:**  The agent maps the memory directory and the MEMORY.md index. This baseline scan allows the agent to identify existing topics and prevents the creation of duplicate files or redundant information pathways.  
2. **Phase 2: Signal Gathering:**  To ensure performance, the agent avoids reading thousands of lines of transcripts. Instead, it performs  **targeted grep-style searches**  of local JSONL logs. It extracts high-value signals such as explicit user corrections ("No, use Fastify"), explicit saves ("Remember this build command"), and recurring architectural themes.  
3. **Phase 3: Consolidation & Semantic Merging:**  The agent merges insights into the relevant topic files. Crucially, it mitigates  **Temporal Entropy**  by converting relative dates (e.g., "yesterday") into absolute ISO-8601 dates (e.g., "2026-03-15"). This ensures that a note like "fix this tomorrow" does not become a permanent, misleading instruction months later.  
4. **Phase 4: Pruning and Indexing:**  The agent enforces the 200-line limit on the MEMORY.md index. It removes pointers to deleted files, resolves contradictions at the source, and reorders the index for optimal retrieval, ensuring the most relevant information is at the top of the context at the start of the next session.

#### 4\. The "Dream" System Prompt & Sub-Agent Mechanics

Consolidation is executed by a forked sub-agent operating under a specialized "Dream" prompt discovered in the source leak.

##### Leaked System Prompt Instructions

"Synthesize what you have learned recently into durable wellorganized memories. Do not exhaustively read transcripts; look only for things you already suspect matter. Keep memory.md under the line limit—it is an index, not a dump. Link to memory files with one-line descriptions. Do not copy full memory into it. Resolve contradictions at the source."

##### The Skeptical Memory Principle

AutoDream implements a  **Check-before-assert**  logic known as "Skeptical Memory." The agent is architected to treat its own memories as "hints" or "suggestions" rather than immutable facts. Before acting on a remembered detail—such as a file path or a function signature—the agent is required to verify its existence against the current codebase. This prevents the agent from hallucinating based on stale context that may have been refactored or deleted since the last consolidation.

#### 5\. Operational Triggers and Background Execution

Consolidation is gated by logic that prevents unnecessary resource consumption while ensuring active projects stay organized.\!IMPORTANT  **Dual-Gate Trigger Requirements**  Automatic consolidation initiates only when both gates are passed:

* **Temporal Gate:**  A minimum of 24 hours must have elapsed since the last consolidation cycle.  
* **Activity Gate:**  A minimum of 5 distinct sessions must have been completed since the last run.Manual triggers are available via natural language commands (e.g., "Consolidate my memory") or the /dream skill, though the skill availability depends on the rollout status of the specific account.

#### 6\. Safety Boundaries and Resource Management

The architecture employs a strict safety layer to protect the host project during background maintenance.

* **Sandboxed Write-Access:**  The consolidation agent has read-only restrictions for all source code, config files, and tests. It possesses write-access exclusively within the .claude/memory/ directory.  
* **Locking Mechanisms:**  To maintain data integrity, the system uses "consolidation lock files." These prevent concurrent dream cycles if multiple agent instances are active on the same project.  
* **Resource Offloading:**  To prevent context window overflow during large searches, the system offloads massive tool results to disk. Only concise "previews" and direct file references are returned to the agent's context, significantly reducing token overhead.

#### 7\. Implementation Benefits & Performance Benchmarks

By delegating maintenance to the AutoDream layer, the architecture yields:

* **Reduced Bloat:**  Only high-signal data survives the pruning phase, keeping the context lean.  
* **Enhanced Recall:**  Resolving contradictions and merging duplicates eliminates the cognitive noise that causes agent "fuzziness."  
* **Temporal Accuracy:**  Absolute dating preserves the timeline of project decisions indefinitely.**Performance Benchmark:**  Analysis of production-level logs shows the architecture is capable of consolidating  **913 sessions of historical data in approximately 8 to 9 minutes** , running entirely in the background without interrupting primary developer workflows.

#### 8\. Future Roadmap: The Evolution of Autonomous Agents

The leaked source maps reveal several advanced features currently in development or limited rollout:

* **KAIROS Always-On Daemon:**  A persistent background process with a  **15-second blocking budget**  for proactive maintenance and log management.  
* **Coordinator Mode:**  A multi-agent orchestration layer that allows a lead agent to delegate specialized tasks to parallel workers, using prompt cache sharing to minimize costs.  
* **Undercover Mode:**  A specialized safety feature used by Anthropic employees to prevent internal model names (e.g.,  **Capybara/Claude 4.6** ,  **Fen/Opus 4.6** , or  **Numbat** ) or internal project code names from appearing in public PR descriptions or commit messages.

