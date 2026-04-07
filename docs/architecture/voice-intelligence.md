# OpenFang Voice Intelligence Architecture

**Date:** 2026-04-06
**Status:** Active — voice pipeline operational on GPU #1 (GTX 1080)

---

## Overview

Voice in OpenFang is a **presentation layer** — the kernel, agent loop, and memory system are completely unchanged. Audio arrives at the WebSocket endpoint, gets transcribed via a local STT service, is fed to the agent as text, and the agent's response is synthesized via a local TTS service and streamed back as audio frames.

Two complementary layers handle voice:

1. **Voice inference services** — standalone Python services exposing OpenAI-compatible HTTP APIs. STT (Whisper) runs on **prtr GTX 1080 #1**. TTS runs on **prtr GTX 1080 #1** (Kokoro, active) or **drtr RTX 2080** (Qwen3-TTS, installed but not active — too slow for real-time). OpenFang calls these over HTTP and does not manage model loading or GPU allocation directly.

2. **Voice transport layer** — implemented in `crates/openfang-api/src/voice.rs` and `ws.rs`. Handles the binary WebSocket protocol, VAD, codec, STT/TTS client calls, sentence buffering, and the full conversation turn loop. TtsClient sends `language: "English"` explicitly. SttClient sends `language: "en"` explicitly.

---

## GPU #1 VRAM Budget

GTX 1080 #1: **8192MB total** (SM 6.1, Pascal)

| Service | Model | Compute | VRAM | Port |
|---------|-------|---------|------|------|
| Kokoro TTS | `kokoro-v1.0.fp16-gpu.onnx` (177MB weights) | FP16 CUDA | ~400MB | 7744 |
| Whisper STT | `distil-whisper/distil-large-v3` | float32 CUDA (CTranslate2) | ~3200MB | 7733 |
| CUDA context | — | — | ~150MB | — |
| **Total** | | | **~3750MB** | **~4400MB headroom** |

Both services are pinned to GPU #1 via `CUDA_VISIBLE_DEVICES=GPU-90117553-c3c8-3546-debd-abb56ca33395` in their systemd unit drop-ins.

**Why GPU #1?** The GTX 970s (GPU #0, #3) are SM 5.2 (Maxwell). CTranslate2 dropped SM 5.2 CUDA kernels in v4.7.1, and ONNX Runtime fp16 Cast nodes fail on SM 5.2. The GTX 1080 (SM 6.1, Pascal) supports all required operations. CTranslate2 float16 compute is not available below SM 7.0 (Volta), so Whisper runs float32 on the 1080.

---

## Inference Services

### Kokoro TTS — port 7744

Uses the `kokoro-onnx` Python package (v0.5.0) with model files from [thewh1teagle/kokoro-onnx releases](https://github.com/thewh1teagle/kokoro-onnx/releases). The package handles phonemization (via espeak-ng), tokenization, and ONNX inference internally.

**Known issue: pronunciation.** The Kokoro model was trained on phonemes from `misaki` (hexgrad's G2P), not espeak-ng. The two phonemizers produce different IPA sequences for the same English text (e.g. misaki `həlˈO` vs espeak `həlˈoʊ` for "hello"). This causes consistent mispronunciation. Switching to misaki via `is_phonemes=True` fixes pronunciation but breaks playback (only 1-2 words play). Root cause under investigation — likely related to how `kokoro_onnx.create()` handles pre-phonemized input differently from its normal text path.

```
/opt/services/kokoro-tts/
├── app.py               ← Flask wrapper around kokoro_onnx.Kokoro
├── models/
│   ├── kokoro-v1.0.fp16-gpu.onnx   (177MB, FP16 GPU model)
│   └── voices-v1.0.bin             (28MB, numpy voice style archive)
└── venv/                ← Python 3.13 venv
    └── (kokoro-onnx, onnxruntime-gpu, soundfile, flask)
```

**Inference pipeline:**
```
text → espeak-ng phonemizer (via phonemizer library)
     → kokoro_onnx.Tokenizer (DEFAULT_VOCAB, 114 phoneme symbols)
     → ONNX InferenceSession(kokoro-v1.0.fp16-gpu.onnx, CUDAExecutionProvider)
         inputs:  input_ids [1, <=512], style [1, 256], speed [1]
         outputs: audio [1, samples]
     → 24kHz WAV
```

The `kokoro-onnx` package internally splits long text into phoneme batches at the MAX_PHONEME_LENGTH (510) boundary, preferring splits at punctuation marks, and trims leading/trailing silence between batches for natural concatenation.

**API:**
```
POST /v1/audio/speech
Body: {"input": "Hello world", "voice": "af_heart", "speed": 1.0}
Returns: audio/wav (24kHz)

GET /health
Returns: {"status": "ok", "model": "kokoro-v1.0.fp16-gpu.onnx", "voices": "voices-v1.0.bin", "loaded": true}
```

**Available voices:** 64 voices across 10 languages. English voices include `af_heart`, `af_bella`, `af_nova`, `am_adam`, `am_michael`, `bf_emma`, `bm_george`, and many more. Prefix convention: `a` = American, `b` = British, `f` = female, `m` = male.

---

### Whisper STT — port 7733

```
/opt/services/whisper-stt/
├── app.py           ← faster-whisper, distil-large-v3, float32 CUDA
└── venv/            → Python 3.12, faster-whisper
```

**Model:** `distil-whisper/distil-large-v3` — 756M params, float32 via CTranslate2.
6.3x faster than Whisper large-v3, within 1% WER on long-form audio.

**Compute type:** float32. CTranslate2 requires SM 7.0+ (Volta) for float16 compute and SM 7.5+ for int8 compute. The GTX 1080 (SM 6.1) supports float32 only.

**Inference pipeline:**
```
WAV/MP3 → faster-whisper WhisperModel (float32, CUDA device 0)
        → VAD filter → beam search (beam_size=5)
        → text + language + duration
```

**API:**
```
POST /v1/audio/transcriptions
Form: file=<audio>, language=<optional>
Returns: {"text": "...", "language": "en", "duration": 3.2, "device": "cuda"}

GET /health
Returns: {"status": "ok", "model": "distil-large-v3", "device": "cuda"}
```

---

### LFM2.5-Audio — port 7722

Liquid AI's 1.5B interleaved audio+text model. **Not currently active.** Reserved for future Phase 8 evaluation as a unified STT+TTS replacement. Service files remain at `/opt/services/lfm25-audio/` but the systemd unit is not enabled.

---

### Qwen3-TTS 1.7B CustomVoice — drtr RTX 2080 (port 7744)

**Status: Installed, evaluated, not active for real-time use.** Service `qwen3-tts.service` exists on drtr but is not the active TTS endpoint.

Alibaba's Qwen3-TTS-12Hz-1.7B-CustomVoice. Best open-source WER scores (1.24 en) of any model fitting on 8GB. Supports 9 premium speakers (Vivian, Ryan, Claire, Alex, Emma, Lucas, Sophia, Daniel, Isabella), 10 languages, voice cloning from 3-second clips, and natural language style instructions.

**Why not active:** 12.9s for 7.8s of audio on first inference, ~5.5s on warm calls — 0.6x to 1.4x realtime without flash-attn or vLLM serving. Unusable for conversational voice chat where sub-second latency is expected.

```
drtr:/opt/services/qwen3-tts/
├── app.py               ← Flask wrapper, OpenAI-compatible /v1/audio/speech
├── qwen_tts/            ← Qwen3-TTS package (cloned from GitHub)
├── venv/                ← Python 3.13, PyTorch+CUDA 12.4, flash-attn 2.8.3
└── (model weights auto-download to HF cache on first load)
```

**VRAM:** 4.2GB in bf16 eager attention (3.7GB headroom on RTX 2080's 8GB).

**Flask wrapper** maps legacy Kokoro voice names to Qwen3 speakers (`af_heart` → Vivian, `am_adam` → Ryan, etc.) for drop-in compatibility with OpenFang's TtsClient.

**To activate:** Change `tts_endpoint` in config.toml to `http://100.64.0.2:7744`, start service on drtr with `sudo systemctl start qwen3-tts`. To deactivate: point back to `http://localhost:7744` (Kokoro on prtr).

**Future paths to real-time:**
- vLLM serving (optimized batched inference)
- 0.6B variant (~1.5GB, ~3x faster, still beats Kokoro/F5-TTS quality)
- Fix flash-attn detection (installed but transformers falls back to eager)

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
  "channels": 1
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
    |       Energy-based VAD (RMS vs. vad_energy_threshold)
    |       +- Speech detected: accumulate pcm_buffer, send 0x30 VadSpeechStart
    |       +- Silence after speech (>= vad_silence_ms): return Transcribe(buffer)
    |              Also force-transcribes at max_utterance_secs
    |
    +- VoiceAction::Transcribe(pcm_buffer)
    |       pcm_to_wav(pcm, 16000) -> WAV bytes
    |       SttClient.transcribe(wav) -> POST /v1/audio/transcriptions -> text
    |       Send JSON text frame: {"type": "voice_transcript", "content": text}
    |
    +- kernel.send_message_streaming(agent_id, voice_message, ...)
    |       voice_message = "[VOICE MODE] ...\n\n" + transcription
    |       StreamEvent::TextDelta -> markdown_to_speakable() -> sentence_buffer
    |       SentenceBuffer splits on [.!?] boundaries
    |
    +- Per sentence:
            TtsClient.synthesize(sentence) -> POST /v1/audio/speech -> WAV
            resample 24kHz -> 16kHz
            encode_pcm_to_frames(pcm, use_opus)
            +- Opus: chunk 320-sample 20ms frames -> encode -> 0x02 AudioDataOut
            +- PCM16: single 0x02 frame with all LE bytes
            Send 0x10 SpeechStart before first frame
            Send 0x11 SpeechEnd after last frame
```

### Fixed: Streaming Delta Space Preservation

`markdown_to_speakable()` previously called `.trim()` on its output, stripping leading whitespace from streaming text deltas. LLM tokens arrive as `" Coming"`, `" through"` — the leading space is a word separator. Trimming it caused the `SentenceBuffer` to concatenate words without spaces (e.g. `Comingthroughloudandclear`), producing garbled TTS output across all models. **Fix:** changed `trim()` to `trim_end()` — leading spaces are preserved, trailing whitespace is still cleaned.

### Known Limitation: Sentence-Level TTS Chunking

The streaming TTS path splits the agent's response at sentence boundaries (`.`, `!`, `?`) and synthesizes each sentence as a separate HTTP call to Kokoro. Each call produces an independent audio segment with its own prosody contour. This causes unnatural cadence and rhythm between sentences — each one starts and ends independently rather than flowing naturally.

Kokoro's internal `_split_phonemes()` method handles multi-sentence text well (splitting only at the 510-phoneme boundary), so batching larger text chunks before TTS would improve naturalness. This requires changes to the `SentenceBuffer` logic in `voice.rs` or the streaming TTS task in `ws.rs`.

### Barge-in (Interrupt)

When the client sends `0x40 Interrupt`:
- Current TTS synthesis is abandoned
- `VoiceSession.reset_to_idle()` clears pcm_buffer and sentence_buffer
- Ready for next utterance immediately

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
| Kokoro models | `/opt/services/kokoro-tts/models/` |
| Kokoro venv | `/opt/services/kokoro-tts/venv/` (Python 3.13) |
| Whisper app | `/opt/services/whisper-stt/app.py` |
| Whisper venv | `/opt/services/whisper-stt/venv/` (Python 3.12) |
| Kokoro systemd | `/etc/systemd/system/kokoro-tts.service` |
| Kokoro GPU pin | `/etc/systemd/system/kokoro-tts.service.d/gpu-pinning.conf` |
| Whisper systemd | `/etc/systemd/system/whisper-stt.service` |
| Whisper GPU pin | `/etc/systemd/system/whisper-stt.service.d/gpu-pinning.conf` |
| Voice transport | `crates/openfang-api/src/voice.rs` |
| WS handler | `crates/openfang-api/src/ws.rs` |
| Dashboard voice UI | `crates/openfang-api/static/js/pages/chat.js` |
| Standalone voice UI | `crates/openfang-api/static/voice.html` |
| Viper repo (Forgejo) | `ssh://git@git.ism.la:6666/rtr/viper.git` |
| Qwen3-TTS app (drtr) | `drtr:/opt/services/qwen3-tts/app.py` |
| Qwen3-TTS systemd (drtr) | `drtr:/etc/systemd/system/qwen3-tts.service` |

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

| GPU | VRAM | CC | UUID | Role | Status |
|-----|------|----|------|------|--------|
| GTX 970 #0 | 4GB | 5.2 | `GPU-6ed2e333...` | Unassigned | Idle |
| GTX 1080 #1 | 8GB | 6.1 | `GPU-90117553...` | **Voice pipeline** (STT + TTS) | Active, ~3.7GB used |
| GTX 1080 #2 | 8GB | 6.1 | `GPU-05b5c553...` | Ollama LLMs | Active |
| GTX 970 #3 | 4GB | 5.2 | `GPU-34b14196...` | Unassigned (prev. voice, SM 5.2 incompatible) | Idle |

### drtr (Director)

| GPU | VRAM | CC | Role | Status |
|-----|------|----|------|--------|
| RTX 2080 | 8GB | 7.5 (Turing, Tensor Cores) | **Qwen3-TTS** (installed, not active) | Idle (prev. vLLM, retired) |

---

## Implementation Status

| Component | Status |
|-----------|--------|
| Binary frame protocol parser/encoder | Complete |
| Opus codec (16kHz mono, 20ms frames) | Complete |
| PCM16 codec (iOS Safari) | Complete |
| Energy-based VAD | Complete |
| SttClient (Whisper HTTP) | Complete |
| TtsClient (Kokoro HTTP, 24->16kHz resample) | Complete |
| SentenceBuffer (split on `.!?` for streaming TTS) | Complete (cadence issue noted) |
| Markdown stripper (for TTS-safe text) | Complete (trim bug fixed — see below) |
| VoiceSession state machine | Complete |
| Barge-in / interrupt handling | Complete |
| WS integration in `ws.rs` | Complete |
| Voice mode context (`[VOICE MODE]` prefix) | Complete |
| Dashboard voice UI (4-state mic button) | Complete |
| Standalone voice UI (`/voice`) | Complete |
| Caddy routing (`vox.ism.la -> :4477`) | Complete |
| Explicit language in TTS/STT requests | Complete |
| kokoro-onnx proper tokenization (espeak-ng) | Complete (pronunciation issue — see note above) |
| Qwen3-TTS on drtr (alternative TTS) | Installed, evaluated — latency too high for real-time |
| LFM2.5-Audio cutover (unified STT+TTS) | Deferred |

---

## History

- **2026-04-01:** Voice transport layer implemented in `voice.rs` / `ws.rs`
- **2026-04-01:** Voice services initially deployed on GPU #3 (GTX 970)
- **2026-04-06:** Discovered SM 5.2 incompatibility — CTranslate2, ONNX Runtime fp16 all fail on Maxwell
- **2026-04-06:** Migrated both services to GPU #1 (GTX 1080, SM 6.1)
- **2026-04-06:** Replaced hand-rolled misaki tokenizer with kokoro-onnx package (espeak-ng phonemizer, proper vocab)
- **2026-04-06:** Downloaded proper model files from thewh1teagle/kokoro-onnx releases
- **2026-04-06:** Added `[VOICE MODE]` context prefix for voice-active agent interactions
- **2026-04-06:** End-to-end voice conversation verified working (STT -> agent -> TTS)
- **2026-04-06:** Qwen3-TTS 1.7B deployed on drtr RTX 2080, evaluated — excellent quality, too slow for real-time (~5-13s/sentence)
- **2026-04-06:** Fixed `markdown_to_speakable()` trim bug — words were concatenated without spaces in streaming deltas
- **2026-04-06:** Added explicit `language` fields to TtsClient ("English") and SttClient ("en")
- **2026-04-06:** Switched back to Kokoro on prtr for real-time use; Qwen3-TTS on drtr available as alternative
