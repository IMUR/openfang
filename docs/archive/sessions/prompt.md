**Goal:** Drive **WebRTC in Rust (`webrtc-rs`)** toward **integration inside OpenFang** (signaling/media paths, agents, daemon/API surfaces). Anything learned from older stacks is **repurposed for OpenFang**, not a commitment to extend Vice or Claw as the primary product.

**Why look back at Vice / iPhone / PJSUA at all:** The **built and installed app on the user’s iPhone** likely already contains most of the hard **iOS-side** work (audio, transport, SIP/WebRTC-adjacent behavior). Records and repos are background to **extract patterns, gaps, and reuse options** for the OpenFang direction—not to continue those projects on their own terms.

**Primary repos / paths (verify they exist on the machine you’re on):**

- **macOS (trtr):** `/Users/trtr/Projects/vice-mobile` — iOS client; prior work on smoke testing, WebSocket/audio, on-device logs (`Documents/vice-mobile.log`) per cluster records.
- **Linux (prtr):** `/home/prtr/Projects/clawdio` — static ES modules, Whisper STT, OpenClaw gateway (see openclaw talk/tts audit records).
- **Linux (prtr):** `/home/prtr/Projects/vice-claw` — related client/agent side; protext was initialized per records.

**Cluster records (NFS):** Search under `/mnt/ops/records/**/*.md` (and any `**/ops/records/**` on the node you use) for:

- `vice-mobile`, `vice-claw`, `clawdio`, `webrtc`, `WHEP`, `SIP`
- **`ipjsua`**, **`pjsua`**, **Swift**, **iPhone** — the **installed ipjsua-swift (or related) app on the phone** is the main reason to search; find any record that names it and map what it implies for **OpenFang + webrtc-rs** (not for reviving Vice-first design).

**Useful prior record pointers (from search):**

- `prtr-records/2026-02-28-vice-mobile-smoke-stability-milestone-prep.md` — vice-mobile diagnostics, iPhone smoke testing.
- `prtr-records/2026-02-27-vice-client-agent-scaffolding.md` — vice-mobile as sibling iOS SIP client.
- `prtr-records/2026-03-08-openclaw-talk-crash-tts-audit.md` — clawdio stack audit.
- `crtr-records/2026-03-07-service-surface-stabilization.md` — note to ignore vice-claw/clawdio internals during a cutover (context only; confirm current intent).
- trtr records on **WebRTC VAD** and **mediamtx WHEP** are adjacent infra, not necessarily the same product line — use only if relevant to signaling/media paths.

**Ask mode / constraints:**  

- Map **what webrtc-rs would live inside in OpenFang** vs what stays on **native iOS** (e.g. reuse from the installed app) vs gateway/service boundaries.  
- List **open questions** (signaling, codecs, TURN/STUN, how much the phone app already covers vs what OpenFang must own).  
- Do **not** assume ipjsua records exist until found; **search first**, then summarize.

**Deliverable for the user:** Short written summary: repo map, record citations (paths + dates), recommended **next step for OpenFang** (e.g. webrtc-rs spike in-tree, FFI boundary, or explicit reuse of the installed iOS app’s capabilities).
