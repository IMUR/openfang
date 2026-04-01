# OpenFang Voice Intelligence Architecture

**Date:** 2026-04-01
**Status:** Active — all three services running on GPU #3 (GTX 970)

---

## Overview

Voice inference runs as three standalone Python services on **GTX 970 #3**, each exposing an OpenAI-compatible HTTP API. OpenFang's voice transport layer (planned in `voice.rs` / `ws.rs`) calls these services over HTTP — it does not manage model loading, GPU allocation, or inference directly.

This separation means:
- Voice models can be updated, restarted, or swapped without touching OpenFang
- The same services can be called by any other cluster process that needs TTS/ASR
- OpenFang's voice layer is pure transport: Opus codec, WebSocket binary frames, HTTP clients to these endpoints

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

## Services

### Kokoro TTS — port 7744

Replaced from PyTorch `kokoro 0.9.4` (~2835MB) to ONNX Runtime GPU (~150MB).

```
/opt/services/kokoro-tts/
├── app.py           ← ONNX implementation (updated)
├── venv-onnx/       ← Python 3.13 venv
│   └── (onnxruntime-gpu, misaki[en], huggingface-hub, soundfile, flask)
└── venv -> venv-onnx  ← symlink (updated)
```

**Inference pipeline:**
```
text → misaki G2P → phoneme token IDs
     → ONNX InferenceSession(model_q8f16.onnx, CUDAExecutionProvider)
         inputs:  input_ids [1, ≤512], style [1, 256], speed [1]
         outputs: audio [1, samples]
     → 24kHz WAV
```

**Model download:** `onnx-community/Kokoro-82M-v1.0-ONNX` → `~/.cache/kokoro-onnx/`
Auto-downloads on first request.

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

Existing service, repinned from **GPU #1** to **GPU #3**.

```
/opt/services/whisper-stt/
├── app.py           ← unchanged (faster-whisper, distil-large-v3, int8)
└── venv/            → venv.old.3.12 (Python 3.12, faster-whisper)
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

New service. Liquid AI's flagship 1.5B interleaved audio+text model.

```
/opt/services/lfm25-audio/
├── app.py           ← ONNX multi-model inference service
└── venv/            ← Python 3.13 venv
    └── (onnxruntime-gpu, transformers, tokenizers, librosa, soundfile, flask)
```

**Model:** `LiquidAI/LFM2.5-Audio-1.5B-ONNX` Q4 quantization.

**Architecture (from config.json):**
```
Lfm2AudioForConditionalGeneration
├── audio_encoder_q4.onnx      Conformer, 128-feat mel@16kHz → latent (115M params)
├── audio_embedding_q4.onnx    maps encoder output for decoder conditioning
├── decoder_q4.onnx            LFM2 1.2B causal LM, vocab=65536, hidden=2048
├── audio_detokenizer_q4.onnx  discrete audio tokens → 8-codebook codes
└── vocoder_depthformer_q4.onnx  8 codebooks → 24kHz waveform (6-layer, dim=1024)
```

Token interleaving: 6 text : 12 audio tokens per cycle.
Preprocessor: 16kHz input, 128 mel bins, 25ms window, 10ms stride.

**TTS pipeline:**
```
text → tokenize → decoder (autoregressive, generates interleaved tokens)
     → filter audio tokens (above vocab midpoint)
     → audio_detokenizer → 8-codebook codes
     → vocoder_depthformer → 24kHz waveform
```

**ASR pipeline:**
```
audio → mel spectrogram (librosa, 128-feat, 16kHz)
      → audio_encoder → hidden states
      → audio_embedding → decoder conditioning
      → decoder (generates text tokens) → detokenize → transcript
```

**Model download:** `LiquidAI/LFM2.5-Audio-1.5B-ONNX` (Q4 files only) → `~/.cache/lfm25-audio-onnx/`
Downloads ~850MB on first start. The service logs exact ONNX tensor I/O specs at startup for debugging.

**API:**
```
POST /v1/audio/speech
Body: {"input": "Hello world", "speed": 1.0}
Returns: audio/wav (24kHz)

POST /v1/audio/transcriptions
Form: file=<audio>
Returns: {"text": "..."}

GET /health
Returns: {"status": "ok", "model": "LiquidAI/LFM2.5-Audio-1.5B-ONNX", "loaded": true}
```

**Note:** LFM2.5-Audio is the Phase 8 upgrade for voice chat — it unifies STT+TTS into one model for lower latency and more natural voice interaction. During Phase 1–7 of the voice plan, Kokoro (7744) and Whisper (7733) are the active services. LFM2.5-Audio (7722) runs in parallel for evaluation and gradual cutover.

---

## Activation History

Activated 2026-04-01. Services were started via:

```bash
sudo systemctl stop kokoro-tts          # freed 2.6GB (old PyTorch Kokoro)
# Updated drop-ins: Kokoro + Whisper repinned to GPU #3, LFM2.5 service installed
sudo systemctl daemon-reload
sudo systemctl start kokoro-tts whisper-stt
sudo systemctl enable --now lfm25-audio
sudo systemctl restart whisper-stt      # forced pickup of new GPU pin (old process was stale)
```

**Current state:**
```
$ systemctl is-active kokoro-tts whisper-stt lfm25-audio
active
active
active

$ curl localhost:7744/health
{"loaded":false,"model":"onnx-community/Kokoro-82M-v1.0-ONNX","status":"ok","variant":"onnx/model_q8f16.onnx"}

$ curl localhost:7733/health
{"device":"uninitialized","model":"distil-large-v3","status":"ok"}

$ curl localhost:7722/health
{"loaded":false,"model":"LiquidAI/LFM2.5-Audio-1.5B-ONNX","quantization":"q4","status":"ok","sub_models":[]}
```

Models are lazy-loaded on first request. GPU #3 sits at ~5MB idle; reaches ~2.5GB once all three are warm.

---

## Integration with OpenFang Voice Layer

The voice transport plan (from `~/.claude/plans/wobbly-crafting-hamming.md`) connects to these services via HTTP at these endpoints:

| Phase | Service | Endpoint | Used For |
|-------|---------|----------|---------|
| 3 | Whisper (7733) | `POST /v1/audio/transcriptions` | PCM16 → text after VAD detects speech end |
| 4 | Kokoro (7744) | `POST /v1/audio/speech` | TextDelta stream → WAV → Opus frames back to client |
| 8 | LFM2.5-Audio (7722) | Both endpoints | Unified STT+TTS, replaces Phases 3+4 |

OpenFang voice config (`~/.openfang/config.toml`, to be added):
```toml
[voice]
enabled        = false          # enable when voice.rs is implemented
stt_endpoint   = "http://localhost:7733"
tts_endpoint   = "http://localhost:7744"
stt_model      = "distil-large-v3"
tts_voice      = "af_heart"
tts_speed      = 1.0
vad_silence_ms = 800
```

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
| LFM2.5 systemd | `/etc/systemd/system/lfm25-audio.service` (pending sudo install) |
| Drop-in staging | `/tmp/voice-services/` |
