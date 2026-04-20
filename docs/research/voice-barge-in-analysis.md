# Voice Barge-In / Interrupt Debugging Session

**Date**: 2026-04-06  
**Session type**: Voice chat debugging (observed behavior)  
**Status**: Unresolved — root cause not yet confirmed via code inspection

---

## Observed Behavior

1. **Full audio playback completes regardless of user speech** — The entire TTS-rendered response plays through even when the user speaks during bot playback. There is no perceptible interruption of audio output.
2. **Interrupts seem to be queued, not executed** — The user's new utterances appear to be transcribed and queued for processing, but the current audio is not cut short.
3. **Post-interrupt state corruption** — After an interrupt attempt, the next voice turn sometimes produces text in the chat UI but no TTS audio. A new voice session resolves this, suggesting a CancellationToken that was cancelled during the interrupt was never properly reset for the next turn.
4. **Timing issues** — Responses sometimes arrive late relative to the user's speech, suggesting the processing pipeline and audio playback pipeline are out of phase.

---

## Architecture Background (from Voice Intelligence doc)

The doc describes three interrupt paths as complete and working:

| Path | Mechanism | Expected Behavior |
|------|-----------|-------------------|
| Client-side barge-in | Client detects speech via VadSpeechStart (0x30) while bot speaks → sends 0x40 Interrupt → flushes audio queue | Immediate audio cutoff |
| Server-side 0x40 | Server receives 0x40 → cancels CancellationToken → aborts voice task | TTS generation stops |
| Server-side VAD | Server detects speech in mic audio during Speaking state → triggers BargeIn | Automatic interrupt without client action |

The client receives TTS audio as WAV frames, decodes them into AudioBuffers, and queues them for playback via Web Audio API.

---

## Hypotheses (ranked by likelihood)

### 1. Client stops listening to mic during playback (HIGH)
**Cause**: `voice.html` and/or `chat.js` may pause microphone capture or disable VAD while TTS audio is playing back. This is a common simplification in voice chat implementations.
**Effect**: Client never detects user speech during playback → never sends 0x40 Interrupt → server never cancels → audio plays to completion.
**Verification**: Check whether `getUserMedia` stream processing / VAD analysis continues running during the `Speaking` state in `voice.html` and `chat.js`.

### 2. Client clears queue but doesn't stop active BufferSourceNode (MEDIUM)
**Cause**: The interrupt handler may flush pending AudioBuffers from the playback queue but fail to call `.stop()` on the currently-playing `AudioBufferSourceNode`. These are two separate operations in the Web Audio API.
**Effect**: Current audio chunk finishes playing (can't be stopped), but no new chunks start. This would partially match the observed behavior but doesn't explain why the *entire* message plays through.
**Verification**: Search for `AudioBufferSourceNode` references in `voice.html`/`chat.js` and verify the interrupt handler calls `.stop()` on the active node.

### 3. CancellationToken not reset between turns (HIGH for bug #3)
**Cause**: When a 0x40 interrupt cancels the CancellationToken, the next voice turn may reuse or inherit a token that's already in a cancelled state.
**Effect**: New LLM response generates text (reaches chat UI via normal message path), but the TTS synthesis branch checks the token, finds it cancelled, and silently skips audio generation.
**Verification**: Check session reset logic in `ws.rs` — specifically whether a new CancellationTokenSource is created for each voice turn or if the old cancelled one is reused.

### 4. Server-side VAD not running during TTS output (MEDIUM)
**Cause**: Server may stop processing incoming mic audio for VAD while TTS frames are being generated/sent.
**Effect**: Server-side barge-in path is never triggered. This would be redundant with hypothesis #1 — if the client isn't sending audio, server VAD has nothing to detect.

---

## Suggested Investigation Order

1. **`voice.html` / `chat.js`** — Check if mic capture and VAD continue during the Speaking state. This is the most likely root cause and the easiest to verify.
2. **`ws.rs`** — Check CancellationToken lifecycle: is a new CTS created per voice turn? Is it properly reset after an interrupt?
3. **`voice.html` / `chat.js`** — Check the interrupt handler for `.stop()` on active BufferSourceNode, not just queue clearing.
4. **Server logs** — Add logging at the 0x40 receive point, CancellationToken cancellation, and TTS dispatch to trace the exact failure path.

---

## Key Insight

The voice intelligence doc describes the architecture as complete, but the actual implementation may have one or more of these gaps:
- Mic capture paused during playback (most likely)
- Queue-only flush without active node stop
- Token lifecycle management across turn boundaries

The post-interrupt silent-response bug strongly suggests the token is not being reset, which would need a fix in `ws.rs`. The inability to interrupt at all strongly suggests the client-side VAD is not running during playback.

---

## Files to Inspect

- `voice.html` — Client-side voice UI, audio playback, mic capture, VAD, interrupt handler
- `chat.js` — Alternative client implementation, same concerns
- `ws.rs` — Server-side WebSocket handler, CancellationToken management, session state machine
