# OpenFang Voice Intelligence Architecture

**Date:** 2026-04-01
**Status:** Active — voice transport implemented, services running on GPU #3 (GTX 970)

---

## Overview

Voice in OpenFang is a **presentation layer** — the kernel, agent loop, and memory system are completely unchanged. Audio arrives at the WebSocket endpoint, gets transcribed via a local STT service, is fed to the agent as text, and the agent's response is synthesized via a local TTS service and streamed back as audio frames.

Two complementary layers handle voice:

1. **Voice inference services** — three standalone Python services on **GTX 970 #3**, each exposing an OpenAI-compatible HTTP API. OpenFang calls these over HTTP and does not manage model loading or GPU allocation directly.

2. **Voice transport layer** — implemented in `crates/openfang-api/src/voice.rs` and `ws.rs`. Handles the binary WebSocket protocol, VAD, codec, STT/TTS client calls, sentence buffering, and the full conversation turn loop.

---

## GPU #3 VRAM Budget

GTX 970 #3: **4096MB total, 3500MB fast segment**

| Service | Model | Quantization | VRAM | Port |
|---------|-------|-------------|------|------|
| Kokoro TTS | `onnx-community/Kokoro-82M-v1.0-ONNX` | q8f16 (86MB weights) | ~150MB | 7744 |
| Whisper STT | `distil-whisper/distil-large-v3` | int8 (CTranslate2) | ~950MB | 7733 |
| LFM2.5-Audio | `LiquidAI/LFM2.5-Audio-1.5B-ONNX` | Q4 (~850MB weights) | ~1000MB | 7722 |
| CUDA context | — | — | ~400MB | — |
| **Total** | | | **~2500MB** | **1000MB headroom** |

All three services are pinned to GPU #3 via `CUDA_VISIBLE_DEVICES=GPU-34b14196-8430-f653-e0c8-7b0b2a8a7cb8` in their systemd units.

---

## Inference Services

### Kokoro TTS — port 7744

Replaced from PyTorch `kokoro 0.9.4` (~2835MB) to ONNX Runtime GPU (~150MB).

```
/opt/services/kokoro-tts/
├── app.py           ← ONNX implementation
├── venv-onnx/       ← Python 3.13 venv
│   └── (onnxruntime-gpu, misaki[en], huggingface-hub, soundfile, flask)
└── venv -> venv-onnx  ← symlink
```

**Inference pipeline:**
```
text → misaki G2P → phoneme token IDs
     → ONNX InferenceSession(model_q8f16.onnx, CUDAExecutionProvider)
         inputs:  input_ids [1, ≤512], style [1, 256], speed [1]
         outputs: audio [1, samples]
     → 24kHz WAV
```

**API:**
```
POST /v1/audio/speech
Body: {"input": "Hello world", "voice": "af_heart", "speed": 1.0}
Returns: audio/wav (24kHz)

GET /health
Returns: {"status": "ok", "model": "onnx-community/Kokoro-82M-v1.0-ONNX", "loaded": true}
```

**Available voices:** `af_heart`, `af_bella`, `af_nicole`, `am_adam`, `am_michael`, `bf_emma`, `bm_george` (+ 20 more)

---

### Whisper STT — port 7733

```
/opt/services/whisper-stt/
├── app.py           ← unchanged (faster-whisper, distil-large-v3, int8)
└── venv/            → Python 3.12, faster-whisper
```

**Model:** `distil-whisper/distil-large-v3` — 756M params, int8 via CTranslate2.
6.3× faster than Whisper large-v3, within 1% WER on long-form audio.

**Inference pipeline:**
```
WAV/MP3 → faster-whisper WhisperModel (int8, CUDA device 0)
        → VAD filter → beam search (beam_size=5)
        → text + language + duration
```

**API:**
```
POST /v1/audio/transcriptions
Form: file=<audio>, language=<optional>
Returns: {"text": "...", "language": "en", "duration": 3.2}

GET /health
Returns: {"status": "ok", "model": "distil-large-v3"}
```

---

### LFM2.5-Audio — port 7722

Liquid AI's 1.5B interleaved audio+text model. Runs in parallel with Kokoro/Whisper for evaluation; planned Phase 8 replacement.

```
/opt/services/lfm25-audio/
├── app.py           ← ONNX multi-model inference service
└── venv/            ← Python 3.13 venv
    └── (onnxruntime-gpu, transformers, tokenizers, librosa, soundfile, flask)
```

**Architecture:**
```
Lfm2AudioForConditionalGeneration
├── audio_encoder_q4.onnx      Conformer, 128-feat mel@16kHz → latent (115M params)
├── audio_embedding_q4.onnx    maps encoder output for decoder conditioning
├── decoder_q4.onnx            LFM2 1.2B causal LM, vocab=65536, hidden=2048
├── audio_detokenizer_q4.onnx  discrete audio tokens → 8-codebook codes
└── vocoder_depthformer_q4.onnx  8 codebooks → 24kHz waveform (6-layer, dim=1024)
```

**API:**
```
POST /v1/audio/speech
Body: {"input": "Hello world", "speed": 1.0}

POST /v1/audio/transcriptions
Form: file=<audio>

GET /health
Returns: {"status": "ok", "model": "LiquidAI/LFM2.5-Audio-1.5B-ONNX", ...}
```

---

## OpenFang Voice Transport Layer

### WebSocket Endpoint

Voice chat uses the existing agent WebSocket: `GET /api/ws/{agent_id}`.

The same WS connection that handles text chat also handles voice — the protocol is multiplexed by message type: JSON text frames for chat, **binary frames** for voice audio.

```
wss://vox.ism.la/api/ws/{agent_id}
     Authorization: Bearer <token>
```

The web UI is served at `https://vox.ism.la/` (GET `/voice` → `voice.html`, embedded in the binary at compile time).

### Binary Frame Protocol

| Byte 0 | Name | Direction | Payload |
|--------|------|-----------|---------|
| `0x01` | AudioDataIn | client→server | Audio frame (Opus or PCM16) |
| `0x02` | AudioDataOut | server→client | Audio frame (Opus or PCM16) |
| `0x10` | SpeechStart | server→client | empty |
| `0x11` | SpeechEnd | server→client | empty |
| `0x20` | SessionInit | client→server | JSON config |
| `0x21` | SessionAck | server→client | JSON `{"session_id":"..."}` |
| `0x30` | VadSpeechStart | server→client | empty (energy threshold crossed) |
| `0x31` | VadSpeechEnd | server→client | empty (silence after speech) |
| `0x40` | Interrupt | client→server | empty (barge-in) |
| `0xF0` | Error | server→client | UTF-8 error string |

### SessionInit Payload

Sent by the client as `0x20` frame payload (JSON):

```json
{
  "sample_rate": 16000,
  "codec": "opus",
  "channels": 1
}
```

- `codec`: `"opus"` (default, uses opus-rs encoder/decoder) or `"pcm16"` (raw little-endian i16; used by iOS Safari via ScriptProcessorNode)
- `sample_rate`: default 16000

### Codec Support

| Codec | Client Capture | Server Processing |
|-------|---------------|-------------------|
| `opus` | MediaRecorder / WebRTC | opus-rs decode → PCM16 |
| `pcm16` | ScriptProcessorNode (iOS Safari compatible) | Direct i16 LE interpretation |

TTS output is resampled from the service's 24kHz to 16kHz before encoding for return. When `codec=opus`, the output is encoded via opus-rs. When `codec=pcm16`, raw bytes are returned.

### Voice Turn Pipeline

```
Client sends 0x01 AudioDataIn frames
    │
    ├─ VoiceSession.decode_audio(data)
    │       Opus: decode packet → Vec<i16>
    │       PCM16: parse LE bytes → Vec<i16>
    │
    ├─ VoiceSession.handle_audio(pcm)
    │       Energy-based VAD (RMS vs. vad_energy_threshold)
    │       ├─ Speech detected: accumulate pcm_buffer, send 0x30 VadSpeechStart
    │       └─ Silence after speech (≥ vad_silence_ms): return Transcribe(buffer)
    │              Also force-transcribes at max_utterance_secs
    │
    ├─ VoiceAction::Transcribe(pcm_buffer)
    │       pcm_to_wav(pcm, 16000) → WAV bytes
    │       SttClient.transcribe(wav) → POST /v1/audio/transcriptions → text
    │       Send JSON text frame: {"type": "voice_transcript", "content": text}
    │
    ├─ kernel.send_message_streaming(agent_id, text, ...)
    │       StreamEvent::TextDelta → pushed into sentence_buffer
    │       SentenceBuffer splits on [.!?] boundaries
    │
    └─ Per sentence:
            TtsClient.synthesize(sentence) → POST /v1/audio/speech → WAV
            resample 24kHz → 16kHz
            encode_pcm_to_frames(pcm, use_opus)
            ├─ Opus: chunk 320-sample 20ms frames → encode → 0x02 AudioDataOut
            └─ PCM16: single 0x02 frame with all LE bytes
            Send 0x10 SpeechStart before first frame
            Send 0x11 SpeechEnd after last frame
```

### Barge-in (Interrupt)

When the client sends `0x40 Interrupt`:
- Current TTS synthesis is abandoned
- `VoiceSession.reset_to_idle()` clears pcm_buffer and sentence_buffer
- Ready for next utterance immediately

### Session State Machine

```
Idle ──[SessionInit]──► Listening
  ▲                          │
  │              [VAD silence]│
  │                          ▼
  │                    Transcribing
  │                          │
  │                [STT complete]
  │                          ▼
  │                    Processing
  │                          │
  │               [LLM response]
  │                          ▼
  └──────────[TTS done]── Speaking
```

---

## Voice Web UI (`/voice`)

The voice page is embedded in the binary at compile time via `include_str!("../static/voice.html")` and served at `GET /voice` (public endpoint — no API token required for the page itself).

**Features:**
- Agent selector dropdown (fetches from `GET /api/agents`)
- API token input (persisted in `sessionStorage`)
- Call / hang up button — connects WebSocket, sends `SessionInit` with `codec: "pcm16"` for iOS Safari compatibility
- Mute button
- Transcript view — user speech via `voice_transcript` events, agent responses via `text_delta` accumulation
- Audio playback — receives PCM16 binary frames, queues sequential playback via `AudioContext`
- Uses `ScriptProcessorNode` for mic capture (deprecated but universally supported on iOS Safari)

**Access:**
```
https://vox.ism.la/             # voice UI
https://vox.ism.la/api/ws/...   # voice + chat WebSocket
https://vox.ism.la/stt/...      # proxied to Whisper on :7733
https://vox.ism.la/tts/...      # proxied to Kokoro on :7744
```

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

Previously pointed to clawdio on `:5544` (dead). Updated 2026-04-01 to route all traffic to OpenFang's port 4477.

---

## OpenFang Voice Configuration (`~/.openfang/config.toml`)

```toml
[voice]
enabled               = true
stt_endpoint          = "http://localhost:7733"
tts_endpoint          = "http://localhost:7744"
stt_model             = "distil-large-v3"
tts_voice             = "af_heart"
tts_speed             = 1.0
vad_silence_ms        = 800
vad_energy_threshold  = 0.01
max_utterance_secs    = 30
```

All fields have defaults; only `enabled = true` is required to activate voice.

---

## Service File Locations

| File | Location |
|------|----------|
| Kokoro app | `/opt/services/kokoro-tts/app.py` |
| Kokoro venv | `/opt/services/kokoro-tts/venv-onnx/` |
| Whisper app | `/opt/services/whisper-stt/app.py` |
| LFM2.5 app | `/opt/services/lfm25-audio/app.py` |
| LFM2.5 venv | `/opt/services/lfm25-audio/venv/` |
| Kokoro systemd | `/etc/systemd/system/kokoro-tts.service` |
| Kokoro GPU pin | `/etc/systemd/system/kokoro-tts.service.d/gpu-pinning.conf` |
| Whisper systemd | `/etc/systemd/system/whisper-stt.service` |
| Whisper GPU pin | `/etc/systemd/system/whisper-stt.service.d/gpu-pinning.conf` |
| LFM2.5 systemd | `/etc/systemd/system/lfm25-audio.service` |
| Voice transport | `crates/openfang-api/src/voice.rs` |
| WS handler | `crates/openfang-api/src/ws.rs` |
| Voice UI | `crates/openfang-api/static/voice.html` |

---

## Implementation Status

| Component | Status |
|-----------|--------|
| Binary frame protocol parser/encoder | ✅ Complete (`voice.rs`) |
| Opus codec (16kHz mono, 20ms frames) | ✅ Complete |
| PCM16 codec (iOS Safari) | ✅ Complete |
| Energy-based VAD | ✅ Complete |
| SttClient (Whisper HTTP) | ✅ Complete |
| TtsClient (Kokoro HTTP, 24→16kHz resample) | ✅ Complete |
| SentenceBuffer (split on `.!?` for streaming TTS) | ✅ Complete |
| Markdown stripper (for TTS-safe text) | ✅ Complete |
| VoiceSession state machine | ✅ Complete |
| Barge-in / interrupt handling | ✅ Complete |
| WS integration in `ws.rs` | ✅ Complete |
| Voice UI (`/voice`) embedded in binary | ✅ Complete |
| Auth: `/voice` page is public | ✅ Complete |
| Caddy routing (`vox.ism.la → :4477`) | ✅ Complete |
| LFM2.5-Audio cutover (unified STT+TTS) | ⏳ Phase 8 — evaluation running |

---

## Activation History

- **2026-04-01:** GPU #3 voice services activated (Kokoro ONNX, Whisper repinned, LFM2.5 new)
- **2026-04-01:** Voice transport layer implemented in `voice.rs` / `ws.rs`
- **2026-04-01:** `/voice` page embedded and `vox.ism.la` Caddy block updated to OpenFang
