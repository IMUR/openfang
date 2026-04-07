# OpenFang Voice Intelligence Architecture

**Date:** 2026-04-07
**Status:** Active — voice pipeline on drtr RTX 2080 (Chatterbox-Turbo TTS + Parakeet TDT STT)

---

## Overview

Voice in OpenFang is a **presentation layer** — the kernel, agent loop, and memory system are completely unchanged. Audio arrives at the WebSocket endpoint, gets transcribed via a local STT service, is fed to the agent as text, and the agent's response is synthesized via a local TTS service and streamed back as audio frames.

Three complementary layers handle voice:

1. **Voice inference services** — standalone Python services on **drtr RTX 2080** exposing OpenAI-compatible HTTP APIs. STT (Parakeet TDT 0.6B, fp16) on port 7733. TTS (Chatterbox-Turbo 350M) on port 7744. Total VRAM: 4.6GB / 8GB. OpenFang calls these over HTTP via Tailscale (~7ms RTT).

2. **Voice transport layer** — implemented in `crates/openfang-api/src/voice.rs`, `vad.rs`, and `ws.rs`. Handles the binary WebSocket protocol, neural VAD (Silero v5 via candle), codec, STT/TTS client calls, sentence buffering, barge-in via CancellationToken, and the full conversation turn loop.

3. **Neural VAD** — Silero VAD v5 loaded via candle (safetensors, CPU inference on prtr i9-9900X). Processes 512-sample chunks at 16kHz, returns speech probability. Falls back to energy-based RMS if model unavailable.

---

## drtr RTX 2080 VRAM Budget

RTX 2080: **8192MB total** (SM 7.5, Turing, Tensor Cores)

| Service | Model | Compute | VRAM | Port |
|---------|-------|---------|------|------|
| Chatterbox-Turbo TTS | ResembleAI/chatterbox (350M) | GPU | ~3220MB | 7744 |
| Parakeet TDT STT | nvidia/parakeet-tdt-0.6b-v2 (600M) | fp16 CUDA | ~1350MB | 7733 |
| **Total** | | | **~4573MB** | **~3619MB headroom** |

Both services co-located on drtr GPU 0 via `CUDA_VISIBLE_DEVICES=0`. Parakeet loads via CPU→fp16→GPU to avoid fp32 VRAM peak during model loading.

**Why drtr?** Consolidates voice on a single node with the best GPU (SM 7.5, Tensor Cores, fp16 native). Frees both prtr GTX 1080s for other workloads. Network latency to drtr via Tailscale is ~7ms — negligible vs the 3-6s latency saved by the new models.

**Previous (retired):** Whisper STT + Kokoro TTS ran on prtr GTX 1080 #1 (3.7GB). Qwen3-TTS 1.7B was evaluated on drtr (5.1GB, too slow). All prtr voice services stopped/disabled.

---

## Inference Services

### Chatterbox-Turbo TTS — drtr port 7744 (ACTIVE)

Resemble AI's Chatterbox-Turbo (350M params). Sub-200ms latency, one-step distilled decoder, zero-shot voice cloning from ~10s reference clip, paralinguistic tags (`[laugh]`, `[sigh]`, `[cough]`). MIT license.

```
drtr:/opt/services/chatterbox-tts/
├── app.py               ← Flask wrapper, OpenAI-compatible /v1/audio/speech
└── .venv/               ← Python 3.13, torch 2.6.0+cu124, chatterbox-tts 0.1.7
```

**API:**
```
POST /v1/audio/speech
Body: {"input": "Hello world", "voice": "default", "speed": 1.0, "language": "English"}
Optional: "reference_audio": "<base64 WAV>" for zero-shot voice cloning
Returns: audio/wav (24kHz)

GET /health
Returns: {"loaded": true, "model": "chatterbox-turbo", "device": "cuda", "vram_mb": 3207}
```

**VRAM:** 3220 MiB. Systemd unit: `chatterbox-tts.service`, `CUDA_VISIBLE_DEVICES=0`.

---

### Parakeet TDT STT — drtr port 7733 (ACTIVE)

NVIDIA Parakeet TDT 0.6B v2 via NeMo. #1 on HuggingFace ASR leaderboard (WER 6.05%), RTFx >2000 (50x faster than Whisper). Apache 2.0 license.

```
drtr:/opt/services/parakeet-stt/
├── app.py               ← Flask wrapper, OpenAI-compatible /v1/audio/transcriptions
└── .venv/               ← Python 3.13, torch 2.6.0+cu124, nemo-toolkit 2.7.2
```

**Optimization:** Model loads to CPU first, converts to fp16, then moves to GPU. This avoids the fp32 VRAM peak (~4.6GB) that caused OOM when co-located with Chatterbox. Final VRAM: ~1350 MiB.

**API:**
```
POST /v1/audio/transcriptions
Form: file=<audio.wav>, model=<ignored>, language="en", response_format="json"
Returns: {"text": "transcribed text"}

GET /health
Returns: {"device": "cuda", "model": "parakeet-tdt-0.6b-v2", "precision": "fp16", "loaded": true}
```

**VRAM:** 1350 MiB. Systemd unit: `parakeet-stt.service`, `CUDA_VISIBLE_DEVICES=0`.

---

### Retired Services

**Kokoro TTS (prtr, port 7744):** Retired. 82M ONNX model, espeak-ng phonemizer mispronounced (trained on misaki G2P). Files remain at `/opt/services/kokoro-tts/`, service disabled.

**Whisper STT (prtr, port 7733):** Retired. distil-large-v3, float32 CTranslate2. Files remain at `/opt/services/whisper-stt/`, service disabled.

**Qwen3-TTS 1.7B (drtr, port 7744):** Retired. Excellent quality (WER 1.24 en) but 5-13s/sentence latency. Service disabled, files at `drtr:/opt/services/qwen3-tts/`.

**LFM2.5-Audio (prtr, port 7722):** Deferred. Liquid AI's 1.5B interleaved model. Service files at `/opt/services/lfm25-audio/`, not enabled.

---

## OpenFang Voice Transport Layer

### WebSocket Endpoint

Voice chat uses the existing agent WebSocket: `GET /api/agents/{agent_id}/ws`.

The same WS connection that handles text chat also handles voice — the protocol is multiplexed by message type: JSON text frames for chat, **binary frames** for voice audio.

### Binary Frame Protocol

| Byte 0 | Name | Direction | Payload |
|--------|------|-----------|---------|
| `0x01` | AudioDataIn | client->server | Audio frame (Opus or PCM16) |
| `0x02` | AudioDataOut | server->client | Audio frame (Opus or PCM16) |
| `0x10` | SpeechStart | server->client | empty |
| `0x11` | SpeechEnd | server->client | empty |
| `0x20` | SessionInit | client->server | JSON config |
| `0x21` | SessionAck | server->client | JSON `{"session_id":"..."}` |
| `0x30` | VadSpeechStart | server->client | empty (energy threshold crossed) |
| `0x31` | VadSpeechEnd | server->client | empty (silence after speech) |
| `0x40` | Interrupt | client->server | empty (barge-in) |
| `0xF0` | Error | server->client | UTF-8 error string |

### SessionInit Payload

Sent by the client as `0x20` frame payload (JSON):

```json
{
  "sample_rate": 16000,
  "codec": "pcm16",
  "channels": 1,
  "voice_clone_ref": "<optional base64-encoded WAV for voice cloning>"
}
```

- `codec`: `"opus"` (uses opus-rs encoder/decoder) or `"pcm16"` (raw little-endian i16; used by iOS Safari via ScriptProcessorNode)
- `sample_rate`: default 16000

### Codec Support

| Codec | Client Capture | Server Processing |
|-------|---------------|-------------------|
| `opus` | MediaRecorder / WebRTC | opus-rs decode -> PCM16 |
| `pcm16` | ScriptProcessorNode (iOS Safari compatible) | Direct i16 LE interpretation |

TTS output is resampled from the service's 24kHz to 16kHz before encoding for return. When `codec=opus`, the output is encoded via opus-rs. When `codec=pcm16`, raw bytes are returned.

### Voice Mode Context

When voice is active, the WS handler prepends a `[VOICE MODE]` instruction to the transcribed text before sending it to the agent loop. This tells the agent its response will be spoken aloud via TTS and to adapt its output accordingly (avoid markdown formatting, code blocks, etc.). The text chat path is unaffected.

The agent receives: `[VOICE MODE] <instructions>\n\n<transcription>`

This is implemented in `ws.rs` at the point where `send_message_streaming()` is called. It does not modify the agent's manifest or stored system prompt.

### Voice Turn Pipeline

```
Client sends 0x01 AudioDataIn frames
    |
    +- VoiceSession.decode_audio(data)
    |       Opus: decode packet -> Vec<i16>
    |       PCM16: parse LE bytes -> Vec<i16>
    |
    +- VoiceSession.handle_audio(pcm)
    |       Silero VAD v5 (candle, CPU) or energy-based fallback
    |       +- SpeechStarted: send 0x30 VadSpeechStart, accumulate pcm_buffer
    |       +- BargeIn: speech during Speaking state → cancel TTS, reset
    |       +- Silence after speech (>= vad_silence_ms): return Transcribe(buffer)
    |              Also force-transcribes at max_utterance_secs
    |
    +- VoiceAction::Transcribe(pcm_buffer)
    |       pcm_to_wav(pcm, 16000) -> WAV bytes
    |       SttClient.transcribe(wav) -> POST drtr:7733/v1/audio/transcriptions -> text
    |       Send JSON text frame: {"type": "voice_transcript", "content": text}
    |
    +- kernel.send_message_streaming(agent_id, voice_message, ...)
    |       voice_message = "[VOICE MODE] ...\n\n" + transcription
    |       StreamEvent::TextDelta -> markdown_to_speakable() -> clause_buffer
    |       ClauseBuffer splits on [,;:.!?\n—] boundaries (30-char min)
    |       voice_task spawned with CancellationToken (non-blocking select! loop)
    |
    +- Per clause (cancellable):
            TtsClient.synthesize(clause) -> POST drtr:7744/v1/audio/speech -> WAV
            resample 24kHz -> 16kHz (rubato sinc)
            encode_pcm_to_frames(pcm, use_opus)
            +- Opus: chunk 320-sample 20ms frames -> encode -> 0x02 AudioDataOut
            +- PCM16: single 0x02 frame with all LE bytes
            Send 0x10 SpeechStart before first frame
            Send 0x11 SpeechEnd after last frame
            Check cancel_token.is_cancelled() between each TTS call and frame send
```

### Neural VAD (Silero v5)

Implemented in `crates/openfang-api/src/vad.rs`. Loads `idle-intelligence/silero-vad-v5-safetensors` from HuggingFace hub via candle-nn (safetensors format, no ONNX runtime needed).

**Architecture:** Input [1, 576] → reflection pad → Conv1d STFT (1→258, k=256, s=128) → magnitude [1, 129, 4] → 4× Conv1d encoder → LSTM (128→128, stateful) → Conv1d decoder → sigmoid → probability ∈ [0, 1].

**Integration:** `VoiceSession` holds an optional `SileroVad`. `detect_speech()` accumulates PCM in a 512-sample buffer and feeds complete chunks to Silero. Falls back to RMS energy when Silero is unavailable. Config: `vad_speech_threshold = 0.5` (Silero), `vad_energy_threshold = 0.01` (fallback).

### Barge-in (Interrupt)

Real barge-in implemented via `tokio_util::sync::CancellationToken` with both client-side and server-side paths:

1. **Client-side barge-in (primary path):** When the client receives VadSpeechStart (0x30) while in `speaking` state (bot is talking), it immediately sends an Interrupt frame (0x40) to the server and flushes its local audio playback queue. This is the most reliable path because it doesn't depend on the server detecting speech through the WS audio stream. Implemented in both `voice.html` (standalone) and `chat.js` (dashboard).

2. **Server-side client interrupt (0x40):** WS handler cancels the `CancellationToken`, aborts the `voice_task` JoinHandle, sends SpeechEnd (0x11), resets session.

3. **Server-side VAD-initiated (automatic):** When `handle_audio()` detects speech during `Speaking` state, returns `VoiceAction::BargeIn`. WS handler cancels TTS, sends SpeechEnd, resets, and begins listening for the new utterance.

4. **Non-blocking select! loop:** The voice_task is spawned but NOT awaited inline. The main WS `select!` loop polls both the receiver and the voice_task, allowing Interrupt messages to be processed while TTS is streaming. Previous implementation blocked the message loop during TTS.

**Client-side audio queue flush:** On receiving SpeechEnd (0x11) or Interrupt confirmation (0x40) from the server, the client clears its `playQueue` and stops playback immediately. This prevents buffered audio from continuing to play after the interrupt.

### Clause-Level TTS Chunking

The `ClauseBuffer` (renamed from `SentenceBuffer`, backward-compatible API) splits on clause boundaries — `,;:.!?\n—` — with a configurable minimum threshold (`tts_min_chunk_chars`, default 30). Sentence-ending punctuation (`.!?`) always splits regardless of threshold. This produces more natural cadence than the previous sentence-only splitting by feeding TTS smaller, more frequent chunks.

Audio is resampled from the TTS service's 24kHz to the WS session's 16kHz using a rubato windowed sinc resampler (`SincFixedIn`, BlackmanHarris2 window, 256-point sinc). Falls back to linear interpolation on error.

### Session State Machine

```
Idle --[SessionInit]--> Listening
  ^                          |
  |              [VAD silence]|
  |                          v
  |                    Transcribing
  |                          |
  |                [STT complete]
  |                          v
  |                    Processing
  |                          |
  |               [LLM response]
  |                          v
  +--------[TTS done]-- Speaking
```

---

## Voice Web UI

Two voice interfaces exist:

1. **Dashboard chat** (`/`) — the main dashboard's chat page has a mic button that activates voice mode inline. Implemented in `static/js/pages/chat.js` with a 4-state submit button (idle->mic, text->send, sending->stop, voice->end-call).

2. **Dedicated voice page** (`/voice`) — a standalone voice-only interface at `GET /voice`, embedded in the binary via `include_str!("../static/voice.html")`. Agent selector, API token input, call/hangup, mute, transcript view.

Both use `ScriptProcessorNode` for mic capture (deprecated but universally supported on iOS Safari) and PCM16 codec by default.

---

## OpenFang Voice Configuration (`~/.openfang/config.toml`)

```toml
[voice]
enabled               = true
stt_endpoint          = "http://100.64.0.2:7733"     # Parakeet TDT on drtr
tts_endpoint          = "http://100.64.0.2:7744"     # Chatterbox-Turbo on drtr
stt_model             = "parakeet-tdt-0.6b-v2"
tts_voice             = "default"
tts_speed             = 1.0
vad_silence_ms        = 500
vad_energy_threshold  = 0.01                          # fallback (energy VAD)
vad_speech_threshold  = 0.5                           # Silero VAD probability threshold
max_utterance_secs    = 30
tts_min_chunk_chars   = 30                            # ClauseBuffer threshold
# tts_voice_clone_ref = "/path/to/reference.wav"     # 10-30s WAV for voice cloning
```

All fields have defaults; only `enabled = true` is required to activate voice.

---

## Service File Locations

| File | Location | Status |
|------|----------|--------|
| Chatterbox-Turbo app | `drtr:/opt/services/chatterbox-tts/app.py` | **Active** |
| Chatterbox systemd | `drtr:/etc/systemd/system/chatterbox-tts.service` | **Active** |
| Parakeet TDT app | `drtr:/opt/services/parakeet-stt/app.py` | **Active** |
| Parakeet systemd | `drtr:/etc/systemd/system/parakeet-stt.service` | **Active** |
| Voice transport | `crates/openfang-api/src/voice.rs` | Active |
| Neural VAD | `crates/openfang-api/src/vad.rs` | Active |
| WS handler | `crates/openfang-api/src/ws.rs` | Active |
| Dashboard voice UI | `crates/openfang-api/static/js/pages/chat.js` | Active |
| Standalone voice UI | `crates/openfang-api/static/voice.html` | Active |
| Kokoro TTS (prtr) | `/opt/services/kokoro-tts/` | Retired, disabled |
| Whisper STT (prtr) | `/opt/services/whisper-stt/` | Retired, disabled |
| Qwen3-TTS (drtr) | `drtr:/opt/services/qwen3-tts/` | Retired, disabled |
| Viper repo (Forgejo) | `ssh://git@git.ism.la:6666/rtr/viper.git` | — |

---

## Caddy Configuration (`crtr:/etc/caddy/Caddyfile`)

`vox.ism.la` proxies to OpenFang on prtr via Tailscale:

```caddy
vox.ism.la {
    encode zstd gzip

    handle_path /stt/* {
        reverse_proxy http://100.64.0.7:7733
    }

    handle_path /tts/* {
        reverse_proxy http://100.64.0.7:7744
    }

    reverse_proxy http://100.64.0.7:4477 {
        header_up Host {host}
        header_up X-Real-IP {remote_host}
    }
}
```

---

## Machine GPU Allocation (Full)

### prtr (Projector)

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| GTX 970 #0 | 4GB | 5.2 | Unassigned | Idle |
| GTX 1080 #1 | 8GB | 6.1 | Freed (prev. Whisper+Kokoro) | Idle |
| GTX 1080 #2 | 8GB | 6.1 | Ollama LLMs | Active |
| GTX 970 #3 | 4GB | 5.2 | Unassigned | Idle |

### drtr (Director)

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| RTX 2080 | 8GB | 7.5 (Turing) | **Voice pipeline** (Chatterbox 3.2GB + Parakeet 1.4GB) | Active, ~4.6GB used, ~3.6GB headroom |

---

## Implementation Status

| Component | Status |
|-----------|--------|
| Binary frame protocol parser/encoder | Complete |
| Opus codec (16kHz mono, 20ms frames) | Complete |
| PCM16 codec (iOS Safari) | Complete |
| Neural VAD (Silero v5, candle, CPU) | Complete |
| Energy-based VAD (fallback) | Complete |
| SttClient (Parakeet TDT HTTP) | Complete |
| TtsClient (Chatterbox HTTP, 24->16kHz resample) | Complete |
| ClauseBuffer (split on `,;:.!?\n—`, 30-char min) | Complete |
| Markdown stripper (for TTS-safe text) | Complete |
| VoiceSession state machine | Complete |
| Barge-in via CancellationToken | Complete (client-initiated + VAD-initiated) |
| Non-blocking voice_task select! loop | Complete |
| WS integration in `ws.rs` | Complete |
| Voice mode context (`[VOICE MODE]` prefix) | Complete |
| Dashboard voice UI (4-state mic button) | Complete |
| Standalone voice UI (`/voice`) | Complete |
| Caddy routing (`vox.ism.la -> :4477`) | Complete |
| Chatterbox-Turbo TTS on drtr | Complete (3.2GB VRAM) |
| Parakeet TDT STT on drtr (fp16) | Complete (1.35GB VRAM) |
| Sinc resampler (rubato, 24→16kHz) | Complete |
| Voice cloning (Chatterbox ref audio) | Complete (config + SessionInit + TtsClient) |

---

## History

- **2026-04-01:** Voice transport layer implemented in `voice.rs` / `ws.rs`
- **2026-04-01:** Voice services initially deployed on GPU #3 (GTX 970)
- **2026-04-06:** Discovered SM 5.2 incompatibility — CTranslate2, ONNX Runtime fp16 all fail on Maxwell
- **2026-04-06:** Migrated both services to GPU #1 (GTX 1080, SM 6.1)
- **2026-04-06:** Replaced hand-rolled misaki tokenizer with kokoro-onnx package
- **2026-04-06:** End-to-end voice conversation verified working
- **2026-04-06:** Qwen3-TTS 1.7B evaluated on drtr — excellent quality, too slow for real-time
- **2026-04-06:** Fixed `markdown_to_speakable()` trim bug (words concatenated without spaces)
- **2026-04-06:** Added explicit `language` fields to TtsClient and SttClient
- **2026-04-07:** Phase 1 — Barge-in via CancellationToken + non-blocking select! loop
- **2026-04-07:** Phase 1 — Silero VAD v5 integrated via candle (safetensors, CPU, ~5ms/chunk)
- **2026-04-07:** Phase 1 — VoiceAction::BargeIn + SpeechStarted variants, VadSpeechStart (0x30) now sent
- **2026-04-07:** Phase 2 — Chatterbox-Turbo 350M deployed on drtr:7744 (3.2GB VRAM, sub-200ms)
- **2026-04-07:** Phase 2 — Parakeet TDT 0.6B deployed on drtr:7733 (fp16, 1.35GB VRAM, RTFx >2000)
- **2026-04-07:** Phase 2 — Parakeet OOM fix: CPU→fp16→GPU loading avoids fp32 peak
- **2026-04-07:** Phase 2 — All prtr voice services retired (Kokoro, Whisper), both GTX 1080s freed
- **2026-04-07:** Phase 2 — Config updated: endpoints → drtr, vad_silence_ms 800→500
- **2026-04-07:** Phase 3 — ClauseBuffer replaces SentenceBuffer (splits on `,;:.!?\n—`, 30-char min)
- **2026-04-07:** Phase 3 — Rubato sinc resampler replaces linear interpolation (24→16kHz)
- **2026-04-07:** Phase 4 — Voice cloning: tts_voice_clone_ref config, SessionInit extension, TtsClient base64 encoding
- **2026-04-07:** Client-side barge-in: voice.html + chat.js send 0x40 Interrupt when VadSpeechStart arrives during speaking state, flush audio queue
