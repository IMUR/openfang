# Best STT/TTS Stack for RTX 2080 — 2026 Edition

> **Objective:** Identify the highest-performing, fully local STT and TTS models that genuinely exploit the RTX 2080's 8 GB VRAM and Turing-generation tensor cores, rather than models designed to run on older or weaker hardware.

***

## Executive Summary

The RTX 2080 has approximately 2.5× the FP32 compute of a GTX 970 and gains Turing tensor cores, making it capable of running 2025–26-era heavyweight transformer TTS and large-turbo STT models at or better than real-time. The recommended strategy is:[^1]

- **STT:** `faster-whisper-large-v3-turbo` (fp16, CTranslate2) or `nvidia/parakeet-tdt-0.6b-v2` via NeMo for English-only.
- **TTS fast lane:** `MOSS-TTS-Realtime` (1.7B) for conversational voice agents — TTFB of 180 ms.[^2]
- **TTS quality lane:** `MOSS-TTS-Local-Transformer` or `Qwen3-TTS-1.7B` for expressive/cloned output.
- **Avoid:** `Voxtral-4B-TTS` — officially requires ≥16 GB VRAM, making it impractical on an 8 GB card.[^3]

***

## STT Options

### Option 1: Faster-Whisper Large-v3-Turbo (Recommended)

`faster-whisper` using CTranslate2 is the most battle-tested local GPU STT engine. The `large-v3-turbo` checkpoint is the best balance for an 8 GB card:[^4][^5]

- **Architecture:** Encoder-decoder (Whisper) with CTranslate2 GPU kernel optimizations.
- **VRAM usage:** ~2537 MB at fp16, ~1545 MB at int8.[^6][^7]
- **Speed:** Transcribed a test corpus in 19.15 s (fp16) vs 52 s for the full large-v3 — roughly 2.7× faster. Whisper Large v3 Turbo runs at 216× real-time speed factor on optimized hardware.[^7][^8][^6]
- **Accuracy:** WER of 1.919% (fp16) on standard benchmarks — better than distil-large-v3 (2.392%).[^6][^7]
- **Streaming:** Supports VAD-guided chunking with 50–100 ms buffer windows for real-time streaming.[^9]

**Recommended config for RTX 2080:**

```python
from faster_whisper import WhisperModel
model = WhisperModel(
    "deepdml/faster-whisper-large-v3-turbo-ct2",
    device="cuda",
    compute_type="float16"
)
```

**Caveats:** The turbo model can hallucinate more on very short clips (<5 s) compared to distil-large-v3; use VAD to avoid feeding it silence.[^6]

***

### Option 2: NVIDIA NeMo Parakeet (English-only, streaming-optimized)

NVIDIA's Parakeet models (0.6B and 1.1B parameters) are built on Fast Conformer with RNNT/CTC decoders, specifically designed for GPU inference.[^10][^11]

- **Architecture:** Fast Conformer (8× depthwise-separable convolution downsampling) with streaming-optimized attention.[^10]
- **VRAM usage:** Parakeet-0.6B streaming mode uses ~4.13 GB GPU memory; streaming-throughput mode ~5.05 GB.[^12]
- **Strengths:** Exceptional accuracy across accents, dialects, and noisy environments; trained on 64,000 hours. Built-in support for VAD (Silero), long-form audio (up to 11 hours in one pass), and streaming inference.[^11][^12][^10]
- **Limitation:** English-only. The NIM containerized version officially requires Compute Capability 8.0+ (Ampere), but the raw NeMo model runs on Turing (RTX 2080 = Compute 7.5).[^12]

**Best for:** English-only agent pipelines where hallucination resistance and noise robustness matter more than multilingual support.

***

### STT Comparison

| Model | VRAM (fp16) | WER | Streaming | Languages | RTX 2080 fit |
|---|---|---|---|---|---|
| faster-whisper large-v3-turbo | ~2.5 GB[^6] | 1.92%[^6] | Yes (VAD) | 99+ | ✅ Comfortable |
| faster-distil-large-v3 | ~2.4 GB[^6] | 2.39%[^6] | Yes | 99+ | ✅ Comfortable |
| faster-whisper large-v3 (full) | ~4.5 GB[^6] | 2.88%[^6] | Yes | 99+ | ✅ OK, less headroom |
| NeMo Parakeet-0.6B (streaming) | ~4.1 GB[^12] | SOTA English | Native | English | ✅ Comfortable |
| NeMo Parakeet-1.1B (streaming) | ~7.2–7.6 GB[^12] | SOTA English | Native | English | ⚠️ Tight |

***

## TTS Options

### Option 1: MOSS-TTS-Realtime (Recommended for Agents)

`MOSS-TTS-Realtime` is purpose-built for interactive voice agents requiring low-latency, multi-turn streaming synthesis.[^13][^14]

- **Architecture:** 1.7B-parameter backbone + 200M-parameter local Transformer.[^15]
- **TTFB:** 180 ms standalone; combined LLM-first-sentence + TTS TTFB is 377 ms.[^2]
- **Context awareness:** Conditions on both text and prior acoustic context across turns, maintaining voice consistency in multi-turn conversations over a 32K context window (~40 minutes).[^13]
- **Cloning:** Zero-shot voice cloning with high speaker similarity maintained across turns.[^14][^13]
- **Output:** 24 kHz audio.[^16]
- **VRAM:** The 1.7B backbone fits comfortably in 8 GB at fp16; exact profiling for 2080 is not published, but the model family is sized for single-GPU deployment.[^17][^16]

**Best for:** The default voice agent response lane — fast, coherent, context-aware.

***

### Option 2: MOSS-TTS-Local-Transformer (Quality Lane)

The `Local` architecture variant emphasizes streaming-oriented performance and stronger speaker preservation.[^16][^2]

- **Architecture:** Global latent + local Transformer design; no delay scheduling, making alignment simpler and streaming-friendly.[^2]
- **Strengths:** Higher objective performance scores, stronger voice cloning fidelity, better streaming alignment than the Delay architecture.[^16][^2]
- **Use case:** Narration, named personas, or anything where voice consistency and naturalness are more important than absolute minimum TTFB.[^2]

***

### Option 3: Qwen3-TTS-1.7B (Expressive, Controllable)

`Qwen3-TTS-1.7B` adds natural-language emotion control on top of solid quality.[^18][^19]

- **VRAM:** ~8 GB (listed minimum: RTX 3080 10 GB; the 2080's 8 GB is borderline).[^19]
- **Streaming latency:** ~97 ms TTFB with streaming enabled.[^19]
- **Controls:** Accepts natural language instructions like "speak happily", "whisper softly" directly in the prompt.[^19]
- **Cloning:** Zero-shot voice cloning from a short reference clip.[^18][^19]
- **RTF on RTX 3090:** 1.7B at RTF ~1.26 without FlashAttention; 30–40% improvement with FlashAttention. On a 2080, expect RTF 1.5–2.0 — slightly slower than real-time for longer utterances.[^18]

**Best for:** Expressive synthesis, stylized personas, or emotionally varied agent outputs where you can tolerate slightly higher latency.

***

### Option 4: Qwen3-TTS-0.6B (Fast, Fits Comfortably)

The smaller Qwen3-TTS variant is a reliable backup when VRAM is shared with a hot STT model.[^18][^19]

- **VRAM:** ~4 GB.[^19]
- **RTF on RTX 3090:** ~0.86 (slightly better than real-time). Expected comfortable real-time on 2080.[^18]
- **Same controls:** Natural-language emotion/style instructions, zero-shot cloning.[^19]
- **Limitation:** Lower naturalness ceiling than 1.7B; still a major step up from Kokoro in expressiveness.[^18]

***

### Option 5: Voxtral-4B-TTS ⚠️ (Not Recommended for RTX 2080)

Mistral's Voxtral-4B-TTS-2603 is currently the top-ranked open-source TTS for quality and multilingual output. However:[^20][^3]

- **Official VRAM requirement:** ≥16 GB (BF16 weights).[^3]
- **Practical workaround:** Running quantized/CPU-offloaded, reported VRAM use dropped to ~3 GB in one test, but this was on the codec encoder, not the full generation pipeline.[^21][^22]
- **Performance benchmarks:** Published on H200, showing 70 ms latency at concurrency=1. Extrapolating to an 8 GB Turing card with offloading would likely push latency to seconds.[^3]

**Verdict:** Wait for a properly quantized GGUF/GPTQ version before attempting on 8 GB.

***

### TTS Comparison

| Model | Params | VRAM | TTFB | Cloning | Streaming | RTX 2080 fit |
|---|---|---|---|---|---|---|
| MOSS-TTS-Realtime | 1.7B[^15] | ~6–7 GB est. | 180 ms[^2] | Zero-shot | Native | ✅ Recommended |
| MOSS-TTS-Local-Transformer | ~1.7B[^2] | ~6–7 GB est. | Short | Zero-shot | Native | ✅ Quality lane |
| Qwen3-TTS-0.6B | 0.6B[^19] | ~4 GB[^19] | ~97 ms[^19] | Zero-shot | Yes | ✅ Comfortable |
| Qwen3-TTS-1.7B | 1.7B[^19] | ~8 GB[^19] | ~97 ms[^19] | Zero-shot | Yes | ⚠️ Tight |
| Voxtral-4B-TTS | 4B[^3] | ≥16 GB[^3] | 70 ms (H200)[^3] | Yes | Yes | ❌ Too large |
| Kokoro-82M | 82M[^23] | <1 GB | <300 ms[^24] | No | Yes | ✅ Overkill — wastes GPU |

***

## Recommended Stack Configuration

For a dedicated RTX 2080 speech node (e.g., as a sidecar to an OpenFang agent OS):

### Always-hot (loaded at startup)

| Role | Model | VRAM budget |
|---|---|---|
| STT | faster-whisper-large-v3-turbo (fp16) | ~2.5 GB |
| TTS fast | MOSS-TTS-Realtime (fp16) | ~3.5 GB est. |
| **Total** | | **~6 GB** |

This leaves ~2 GB of headroom for CUDA context, activations, and buffers, which is safe for sustained single-user streaming.

### On-demand (loaded per request, or promoted if VRAM allows)

| Role | Model | Notes |
|---|---|---|
| TTS expressive | MOSS-TTS-Local-Transformer | Swap in for narration/persona |
| TTS controllable | Qwen3-TTS-0.6B | Emotion-directed synthesis |
| STT alternative | NeMo Parakeet-0.6B | English-only, lower hallucination rate |

### Latency budget for a full turn (RTX 2080 estimate)

| Stage | Latency |
|---|---|
| VAD endpoint detection | ~50–100 ms[^9] |
| faster-whisper-large-v3-turbo transcription | ~150–250 ms (short utterance)[^6][^7] |
| Network to agent machine + inference | Agent-dependent |
| MOSS-TTS-Realtime TTFB (first audio chunk) | 180 ms[^2] |
| **Total to first audio (excluding LLM)** | **~380–530 ms** |

With LLM first-sentence latency on the separate machine included, the MOSS-TTS team measured a combined 377 ms when the LLM first sentence arrives quickly.[^2]

***

## Architecture for Split Machines

The RTX 2080 node runs as a dedicated streaming speech service; the agent/orchestration machine (OpenFang) handles LLM inference, memory, and decision logic.

```
[Mic/Client]
     │  PCM (16 kHz, 100-200 ms chunks)
     ▼
[RTX 2080 Node — Speech Coprocessor]
  ┌──────────────────────────────────────────┐
  │  STT: faster-whisper-large-v3-turbo      │
  │       ↓ partial + final transcripts      │
  │  VAD: silero-vad (pre-endpointing)       │
  └──────────────────────────────────────────┘
     │  text (partial + final) + timestamps
     ▼
[Agent Machine — OpenFang]
  ┌──────────────────────────────────────────┐
  │  LLM inference + orchestration           │
  │  Memory / RAG / tool use                 │
  │  Voice policy: fast | expressive | clone │
  └──────────────────────────────────────────┘
     │  text + voice_policy_id
     ▼
[RTX 2080 Node — TTS]
  ┌──────────────────────────────────────────┐
  │  fast:       MOSS-TTS-Realtime           │
  │  expressive: MOSS-TTS-Local-Transformer  │
  │  controlled: Qwen3-TTS-0.6B             │
  └──────────────────────────────────────────┘
     │  PCM chunks (24 kHz, streamed)
     ▼
[Client / Audio Output]
```

### API surface for the speech node

Expose three endpoints from the 2080 box:

- `/stt/stream` — bidirectional WebSocket; PCM in → partial/final transcripts out
- `/tts/fast` — streaming HTTP or WebSocket; text in → PCM chunks out (MOSS-TTS-Realtime)
- `/tts/expressive` — same protocol; text + optional voice ref in → PCM out (Local-Transformer or Qwen3-TTS)

***

## Key Trade-offs

### Non-autoregressive vs. autoregressive TTS

Non-autoregressive (NAR) models like Kokoro are faster per token but produce less natural prosody. Autoregressive models (MOSS-TTS, Qwen3-TTS, Voxtral) can sound dramatically more natural but require streaming to reduce TTFB. Research shows NAR architectures deliver 30–55% lower synthesis latency than autoregressive equivalents, but with expressiveness limitations.[^25]

### Streaming accuracy vs. latency

Streaming TTS operates with 5–20× less context than batch processing, which can degrade pronunciation accuracy on alphanumeric strings, phone numbers, and complex entity names. For conversational agent utterances (typically simple prose), this is not a practical problem. Route entity-heavy content (e.g., reading back IDs, addresses) through a small batch-mode TTS call if accuracy matters more than speed.[^25]

### VRAM allocation policy

Never load all three TTS models simultaneously on an 8 GB card. A practical policy:
- Keep STT + fast TTS always hot (~6 GB combined).
- Load expressive TTS on-demand when a "persona" or "narration" flag arrives from the agent.
- Use Python `torch.cuda.empty_cache()` between model swaps and allocate in fp16 throughout.

***

## Model Maturity Notes

| Model | Maturity | Known Issues |
|---|---|---|
| faster-whisper large-v3-turbo | Production-stable[^4][^7] | Hallucination on very short/silent clips — mitigate with VAD[^6] |
| NeMo Parakeet-0.6B | Production (NVIDIA-backed)[^10] | English-only; NIM container officially requires CC 8.0[^12] |
| MOSS-TTS-Realtime | Released Feb 2026, production-oriented[^13][^14] | Limited community benchmarks on Turing-gen GPUs specifically |
| MOSS-TTS-Local-Transformer | Released alongside Realtime[^2][^16] | Less community profiling than Realtime variant |
| Qwen3-TTS-1.7B | Requires careful VRAM management on 8 GB[^18][^19] | RTF may exceed 1.0 on 2080 without FlashAttention |
| Qwen3-TTS-0.6B | Comfortable on 8 GB[^19] | Lower naturalness ceiling than 1.7B |
| Voxtral-4B-TTS | Released March 2026[^3] | Requires ≥16 GB VRAM officially; community workarounds in progress[^22] |

---

## References

1. [GeForce RTX 2080 vs GTX 970 - Technical City](https://technical.city/en/video/GeForce-GTX-970-vs-GeForce-RTX-2080) - RTX 2080 outperforms GTX 970 by an impressive 93% based on our aggregate benchmark results. Contents...

2. [OpenMOSS-Team/MOSS-TTS - Hugging Face](https://huggingface.co/OpenMOSS-Team/MOSS-TTS) - MOSS-TTS delivers state-of-the-art quality while providing the fine-grained controllability and long...

3. [mistralai/Voxtral-4B-TTS-2603 - Hugging Face](https://huggingface.co/mistralai/Voxtral-4B-TTS-2603) - Due to size and the BF16 format of the weights - Voxtral-4B-TTS-2603 can run on a single GPU with >=...

4. [Faster Whisper transcription with CTranslate2 - GitHub](https://github.com/SYSTRAN/faster-whisper) - faster-whisper is a reimplementation of OpenAI's Whisper model using CTranslate2, which is a fast in...

5. [OpenAI Whisper Audio Transcription Benchmarked on 18 GPUs](https://www.tomshardware.com/news/whisper-audio-transcription-gpus-benchmarked) - It can provide substantially faster than real-time transcription of audio via your GPU, with the ent...

6. [Benchmark faster whisper turbo v3 · Issue #1030 - GitHub](https://github.com/SYSTRAN/faster-whisper/issues/1030) - Here's the time and memory usage that are required to transcribe 13 minutes of audio using different...

7. [deepdml/faster-whisper-large-v3-turbo-ct2 · Speed benchmark](https://huggingface.co/deepdml/faster-whisper-large-v3-turbo-ct2/discussions/3) - I would like to know if this model is actually faster than the original model. Could you put some re...

8. [Whisper Large v3 Turbo – Fast Speech Recognition Now on Groq](https://groq.com/blog/whisper-large-v3-turbo-now-available-on-groq-combining-speed-quality-for-speech-recognition) - At 216x real-time speed factor it is faster than Whisper Large v3 ... v3 Turbo clocks in at a speed ...

9. [Fastest Whisper v3 Turbo - Serving Millions of Requests at 1300 ...](https://simplismart.ai/blog/fastest-whisper-v3-turbo-serving-millions-of-requests-at-1300-real-time-with-simplismart) - Discover how Simplismart's optimizations supercharge Whisper v3 Turbo to achieve 1300× real-time tra...

10. [Pushing the Boundaries of Speech Recognition with NVIDIA NeMo ...](https://developer.nvidia.com/blog/pushing-the-boundaries-of-speech-recognition-with-nemo-parakeet-asr-models/) - These state-of-the-art ASR models, developed in collaboration with Suno.ai, transcribe spoken Englis...

11. [does nvidia/parakeet-tdt-0.6b-v2 tend to give better results than ...](https://www.reddit.com/r/MachineLearning/comments/1pno4dj/d_people_who_work_with_asr_models_does/) - I have a work stream right now that invoves building around nvidia/parakeet for audio transcription ...

12. [NVIDIA ASR NIM Support Matrix — NVIDIA Speech NIM Microservices](https://docs.nvidia.com/nim/speech/latest/reference/support-matrix/asr.html) - The NVIDIA ASR NIM requires an NVIDIA GPU with a Compute Capability of 8.0 or higher and at least 16...

13. [OpenMOSS-Team/MOSS-TTS-Realtime - Hugging Face](https://huggingface.co/OpenMOSS-Team/MOSS-TTS-Realtime) - It is designed for interactive voice agents that require low-latency, continuous speech generation a...

14. [MOSS-TTS Deep Dive: The Production-Grade Open-Source Voice ...](https://www.communeify.com/en/blog/moss-tts-production-grade-ai-audio-factory-guide/) - MOSS-TTS is now open-source! This production-grade AI audio solution features 5 core models, support...

15. [MOSS-TTS/moss_tts_realtime/README.md at main - GitHub](https://github.com/OpenMOSS/MOSS-TTS/blob/main/moss_tts_realtime/README.md) - MOSS-TTS-RealTime is built on a scalable architecture consisting of a 1.7B-parameter backbone and a ...

16. [[2603.18090] MOSS-TTS Technical Report - arXiv](https://arxiv.org/abs/2603.18090) - Built on MOSS-Audio-Tokenizer, a causal Transformer tokenizer that compresses 24 kHz audio to 12.5 f...

17. [MOSS-TTS Family: 5 Open-Source Speech Models for Commercial ...](https://www.linkedin.com/posts/pasha-shaik_moss-tts-family-just-dropped-5-production-ready-activity-7427446497568366592-xGBf) - MOSS-TTS Family just dropped — 5 production-ready speech models in one open-source release. From MOS...

18. [Qwen3-TTS: The Complete 2026 Guide to Open-Source Voice ...](https://dev.to/czmilo/qwen3-tts-the-complete-2026-guide-to-open-source-voice-cloning-and-ai-speech-generation-1in6) - GTX 1080 (8GB VRAM): ... For production use, RTX 3090 or better is recommended. The 0.6B model can r...

19. [Qwen3-TTS Voice Cloning | Guides - Clore.ai](https://docs.clore.ai/guides/audio-and-voice/qwen3-tts) - It features natural language emotion control ("speak happily", "whisper softly"), streaming with 97m...

20. [Best Open-Source TTS in 2026: 5 Models, Ranked by Quality](https://findskill.ai/blog/best-open-source-tts-2026/) - Best Open-Source TTS in 2026: 5 Models, Ranked by Quality · 1. Voxtral TTS (Mistral) — The New Bench...

21. [How to Install Voxtral 4B TTS 2603 with 9-Language Demo?](https://sonusahani.com/blogs/install-voxtral-4b-tts) - The runtime was light in practice. The model used just over 3 GB of VRAM during synthesis. You can a...

22. [You actually don't need the Voxtral Codec's encoder to get codes for ...](https://www.reddit.com/r/LocalLLaMA/comments/1scvud4/you_actually_dont_need_the_voxtral_codecs_encoder/) - You don't need hours of GPU training to train your own Codec instead of the missing on in Voxtral TT...

23. [hexgrad/Kokoro-82M - Hugging Face](https://huggingface.co/hexgrad/Kokoro-82M) - Kokoro is an open-weight TTS model with 82 million parameters. Despite its lightweight architecture,...

24. [12 Best Open-Source TTS Models Compared (2025) - Inferless](https://www.inferless.com/learn/comparing-different-text-to-speech---tts--models-part-2) - Kokoro-82M emerges as the clear winner for speed, consistently processing texts in under 0.3 seconds...

25. [Streaming TTS Latency Tradeoff: Real-Time Accuracy Loss 2026](https://deepgram.com/learn/streaming-tts-latency-accuracy-tradeoff-2026) - Streaming TTS loses 5-20x context vs batch processing, causing pronunciation failures on alphanumeri...

