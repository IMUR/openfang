# Exploration: Cognitive Architecture

_Hypotheses and open questions regarding the implementation of a persistent mind._

---

## The Premise: Knowledge as Provisional Belief

What the mind calls "knowledge" is typically a set of assumptions and beliefs treated _as if_ they were true. This is a pragmatic requirement for functioning; constant doubt leads to paralysis.

This architecture explores how an LLM can operate on a similar premise: treating its current best approximations as operational truth until they are challenged.

---

## Core Hypotheses

### 1. The Cognitive Tension (Assertion vs. Inquiry)

Instead of discrete "modes" that the system switches between, we are exploring a constant _cognitive tension_:

* **Assertion (Stability):** The default state. The reconciled self-model is treated as operational truth. The system acts confidently on its current understanding.
* **Inquiry (Destabilization):** Not a mode, but a surrender. A belief can no longer be held with stability due to pragmatic triggers:
  * A contradiction that cannot be ignored.
  * New evidence that destabilizes a previously stable understanding.
  * A correction from a trusted source.

**Open Question:** How is this tension quantified? Is it a threshold of contradictory evidence, or an LLM-driven metacognitive evaluation of confidence versus the cost of being wrong?

### 2. Two Parallel Tracks (Immutable vs. Reconciled)

We are testing the idea that cognitive flow requires two distinct representations of memory:

* **The Immutable Track (Epistemic History):** A permanent, append-only record of every state the system has ever been in. It records what was believed, when, what triggered a change, and the reasoning behind it. This is a grounding mechanism—an anti-human perfection of memory provenance.
* **The Reconciled Track (The Self-Model):** The system's living, current-best-understanding of itself in relation to its user. This is what gets loaded into the LLM's context.

**The Fundamental Fork (Open Question):** Is the Reconciled Track a _derivation_ (recomputed on demand from the immutable log) or a _cache_ (materialized and incrementally updated)?

### 3. Subsystem Analogies

We are exploring specific technical primitives as analogs for human memory systems:

* **Trace Store ↔ Episodic Memory:** What if episodic memory worked like an append-only log with bitemporal timestamps (when it happened vs. when the system learned it)?
* **Schema Store ↔ Semantic Memory:** What if decontextualized, "crystallized" facts were stored in a hash-based O(1) lookup table (inspired by the DeepSeek Engram principle)?
* **Similarity Index ↔ Cue-Dependent Retrieval:** What if "this reminds me of..." recall is best handled by vector embeddings and hierarchical search?
* **Context Buffer ↔ Working Memory:** What if the LLM's context window is managed via a tiered loading strategy (L0: core identity, L1: session context, L2: full detail)?

**Open Question:** Are these the right mappings? For example, does a vector store adequately capture the associative nature of human recall, or do we rely too heavily on it because it is the current industry standard?

### 4. Systemic Behaviors

* **Consolidation (Background Reorganization):** Exploring idle-time processing to scan the Trace Store, promote stable patterns to the Schema Store, and reconcile contradictions.
  * _Open Question:_ What triggers consolidation? Is it purely time-based (like sleep), or event-based (after $X$ new experiences)?
* **Reconsolidation (Update on Retrieval):** Human memories become labile and can change when recalled. If our Trace Store is immutable, how do we model reconsolidation?
  * _Hypothesis:_ On retrieval, emit a new transaction that captures the updated understanding, preserving the original but creating a "reconsolidated" version.

---

## The Orchestrator

The orchestrator is hypothesized to be the memory management layer—the prefrontal cortex—not an agent. It doesn't have autonomous goals or tool-calling loops; it routes queries, manages the Context Buffer, and records experiences.

**Open Question:** How complex should the orchestrator be? A rule-based router is tractable but brittle. An LLM-powered intent analyzer is flexible but expensive and potentially unpredictable. Where is the balance?
