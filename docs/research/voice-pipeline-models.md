# Voice Pipeline Model Reference

## Text-to-Speech (TTS) — RTX 2080 (8GB VRAM)

| Model | Parameters | VRAM Est. | Speed | Quality | Notes |
|-------|-----------|-----------|-------|---------|-------|
| Kokoro | 82M | ~1–2GB | ⚡⚡⚡ | Good | Ultra-lightweight, great for always-on use |
| StyleTTS 2 | ~200M | ~2–4GB | ⚡⚡⚡ | Very Good | Fast inference, natural prosody |
| Chatterbox-Turbo | ~350M | ~3–4GB | ⚡⚡⚡ | Very Good | Low-latency, conversational |
| Chatterbox | ~500M | ~4–5GB | ⚡⚡ | Excellent | Expressive, top-tier English quality |
| CosyVoice 2.0 | ~500M | ~4GB | ⚡⚡⚡ | Very Good | Optimized for streaming latency |
| Qwen3-TTS 0.6B | 600M | ~4–5GB | ⚡⚡ | Very Good | Efficient, English-capable |
| F5-TTS | ~500M | ~4–5GB | ⚡⚡ | Very Good | Zero-shot capable, fast |
| Qwen3-TTS 1.7B | 1.7B | ~7–8GB | ⚡ | Excellent | Best quality, tight on 8GB VRAM |

---

## Speech-to-Text (STT) — Heavy / High Accuracy (Home Lab)

| Model | Parameters | Best For | Notes |
|-------|-----------|----------|-------|
| Canary Qwen 2.5B | 2.5B | Max English accuracy | Top benchmark scores |
| IBM Granite Speech 3.3 8B | 8B | Enterprise-grade English ASR | Heavy, needs dedicated GPU |
| Whisper Large V3 | 1.5B | Multilingual + English accuracy | Gold standard |
| Whisper Large V3 Turbo | ~800M | Speed + accuracy balance | Fast throughput |
| Distil-Whisper Large V3 | ~756M | Real-time English STT | Great for conversational pipelines |
| Kyutai STT 2.6B | 2.6B | Local high-accuracy deployment | Solid performance |
| Parakeet TDT | ~600M | Ultra low-latency streaming | Best for real-time chunked audio |
| Faster-Whisper + Distil-Large V3 | ~756M | Optimized inference | CTranslate2 backend, very efficient |

---

## Concurrent / Lightweight STT — Always-On / Browser / Turn-Taking

| Model / Tool | Runs In Browser | Offline | Notes |
|-------------|----------------|---------|-------|
| Vosk | ✅ (via WASM) | ✅ | Lightweight, good for always-on listening |
| Whisper (WASM/ONNX) | ✅ | ✅ | Heavier but accurate, WebGPU accelerated |
| Web Speech API | ✅ | ❌ | Native browser, cloud-dependent, zero setup |
| Moonshine | ✅ (edge) | ✅ | Designed for edge/mobile, very fast |
| Kaldi (adapted) | Partial | ✅ | Low-resource environments, complex setup |
| Mozilla DeepSpeech | ✅ | ✅ | Legacy but stable, simple deployment |

---

## Recommended Pipeline Layout

| Role | Model | Hardware |
|------|-------|----------|
| Always-on lightweight STT (turn-taking) | Vosk or Moonshine (browser/WASM) | Client browser |
| High-accuracy STT (main processing) | Distil-Whisper or Parakeet TDT | RTX 2080 or 1080s |
| TTS (conversational, real-time) | Chatterbox-Turbo or CosyVoice 2.0 | RTX 2080 |
| Heavy STT (optional, max accuracy) | Canary Qwen 2.5B | RTX 2080 dedicated |
