/**
 * OpenFang Voice Client — shared module for voice chat.
 *
 * Single implementation of: mic capture, audio playback, barge-in VAD,
 * WS binary protocol, and session state machine.
 *
 * Consumed by the dashboard chat page via `static/js/pages/chat.js`.
 *
 * Usage:
 *   var vc = new VoiceClient({
 *     ws: websocketInstance,         // or sendBinary: function(buf) { ... }
 *     sampleRate: 16000,
 *     onStateChange: function(state) { ... },
 *     onTranscript: function(text) { ... },
 *     onTextDelta: function(text) { ... },
 *     onError: function(msg) { ... },
 *     onSystemMessage: function(msg) { ... },
 *     bargeInThreshold: 0.008,       // RMS energy threshold for barge-in
 *     bargeInCooldownMs: 2000,
 *   });
 *   vc.start(agentId);   // opens mic, sends SessionInit
 *   vc.stop();           // closes mic, cleans up
 */

var VoiceClient = (function() {
  'use strict';

  var SAMPLE_RATE = 16000;
  var FRAME_SAMPLES = 512;

  // --- Constructor ---

  function VoiceClient(opts) {
    this._opts = opts || {};
    this._sampleRate = opts.sampleRate || SAMPLE_RATE;

    // WS send abstraction: either a raw WebSocket or a sendBinary function
    this._sendBinary = opts.sendBinary || null;
    this._ws = opts.ws || null;

    // Callbacks
    this._onStateChange = opts.onStateChange || function() {};
    this._onTranscript = opts.onTranscript || function() {};
    this._onTextDelta = opts.onTextDelta || function() {};
    this._onError = opts.onError || function() {};
    this._onSystemMessage = opts.onSystemMessage || function() {};
    this._onSessionAck = opts.onSessionAck || function() {};

    // Barge-in config
    this._bargeInThreshold = opts.bargeInThreshold || 0.008;
    this._bargeInCooldownMs = opts.bargeInCooldownMs || 2000;

    // State
    this._state = 'idle'; // idle | connected | listening | speaking
    this._ttsPlaying = false;
    this._bargeInCooldown = false;
    this._muted = false;

    // Audio
    this._audioCtx = null;
    this._mediaStream = null;
    this._processorNode = null;
    this._playQueue = [];
    this._playing = false;
    this._activeSource = null;
    this._currentText = '';
  }

  // --- Public API ---

  VoiceClient.prototype.start = function() {
    var self = this;
    this._audioCtx = new (window.AudioContext || window.webkitAudioContext)({ sampleRate: this._sampleRate });

    // Resume suspended AudioContext (iOS)
    var resumePromise = this._audioCtx.state === 'suspended'
      ? this._audioCtx.resume()
      : Promise.resolve();

    return resumePromise.then(function() {
      return navigator.mediaDevices.getUserMedia({
        audio: { sampleRate: self._sampleRate, channelCount: 1, echoCancellation: true, noiseSuppression: true }
      });
    }).then(function(stream) {
      self._mediaStream = stream;
      var source = self._audioCtx.createMediaStreamSource(stream);

      self._processorNode = self._audioCtx.createScriptProcessor(FRAME_SAMPLES, 1, 1);
      self._processorNode.onaudioprocess = function(e) {
        self._handleAudioProcess(e);
      };

      source.connect(self._processorNode);
      self._processorNode.connect(self._audioCtx.destination);

      // Send SessionInit (PCM16 mode)
      var init = JSON.stringify({ sample_rate: self._sampleRate, codec: 'pcm16', channels: 1 });
      var initBytes = new TextEncoder().encode(init);
      var frame = new Uint8Array(1 + initBytes.length);
      frame[0] = 0x20;
      frame.set(initBytes, 1);
      self._send(frame.buffer);

      self._setState('connected');
    });
  };

  VoiceClient.prototype.stop = function() {
    this.stopPlayback();
    if (this._processorNode) { this._processorNode.disconnect(); this._processorNode = null; }
    if (this._mediaStream) { this._mediaStream.getTracks().forEach(function(t) { t.stop(); }); this._mediaStream = null; }
    if (this._audioCtx) { this._audioCtx.close(); this._audioCtx = null; }
    this._setState('idle');
  };

  VoiceClient.prototype.mute = function() { this._muted = true; };
  VoiceClient.prototype.unmute = function() { this._muted = false; };
  VoiceClient.prototype.isMuted = function() { return this._muted; };
  VoiceClient.prototype.state = function() { return this._state; };
  VoiceClient.prototype.isTtsPlaying = function() { return this._ttsPlaying; };

  /** Manually send interrupt (e.g. from a button). */
  VoiceClient.prototype.interrupt = function() {
    if (!this._ttsPlaying) return;
    this._doBargeIn();
  };

  /** Handle an incoming binary WS frame. Call this from your WS onmessage. */
  VoiceClient.prototype.handleBinaryFrame = function(data) {
    var bytes = new Uint8Array(data);
    if (bytes.length < 1) return;
    var type = bytes[0];
    var payload = bytes.slice(1);

    switch (type) {
      case 0x02: // AudioDataOut — PCM16 LE
        var pcm = new Int16Array(payload.buffer, payload.byteOffset, payload.byteLength / 2);
        this._queuePlayback(pcm);
        break;

      case 0x10: // SpeechStart
        this._ttsPlaying = true;
        this._currentText = '';
        this._setState('speaking');
        break;

      case 0x11: // SpeechEnd — let audio drain naturally
        this._ttsPlaying = false;
        // Don't stopPlayback — let the queue finish. State transitions
        // to 'connected' when drainPlayQueue runs out of buffers.
        if (this._currentText.trim()) {
          this._onTextDelta(null); // signal end-of-response
        }
        this._pollDrainComplete();
        break;

      case 0x21: // SessionAck
        try {
          var ack = JSON.parse(new TextDecoder().decode(payload));
          this._onSessionAck(ack);
        } catch(e) {}
        break;

      case 0x30: // VadSpeechStart
        this._onSystemMessage('Listening...');
        // If server VAD detects speech (barge-in or otherwise), instantly stop local playback
        if (this._ttsPlaying || this._playing) {
          this.stopPlayback();
        }
        break;

      case 0x31: // VadSpeechEnd
        this._onSystemMessage('Transcribing...');
        break;

      case 0xF0: // Error
        var errMsg = new TextDecoder().decode(payload);
        this._onError(errMsg);
        break;
    }
  };

  // --- Audio Playback ---

  VoiceClient.prototype._queuePlayback = function(pcmInt16) {
    this._playQueue.push(pcmInt16);
    if (!this._playing) this._drainPlayQueue();
  };

  VoiceClient.prototype.stopPlayback = function() {
    this._playQueue = [];
    this._playing = false;
    this._ttsPlaying = false;
    if (this._activeSource) {
      try { this._activeSource.onended = null; this._activeSource.stop(); } catch(e) {}
      this._activeSource = null;
    }
  };

  VoiceClient.prototype._drainPlayQueue = function() {
    var self = this;
    if (!this._playQueue.length || !this._audioCtx) {
      this._playing = false;
      this._activeSource = null;
      // If TTS is done and queue is empty, transition to connected
      if (!this._ttsPlaying) {
        this._setState('connected');
      }
      return;
    }
    this._playing = true;
    var pcm = this._playQueue.shift();
    var float32 = new Float32Array(pcm.length);
    for (var i = 0; i < pcm.length; i++) {
      float32[i] = pcm[i] / 32768.0;
    }
    var buf = this._audioCtx.createBuffer(1, float32.length, this._sampleRate);
    buf.getChannelData(0).set(float32);
    var src = this._audioCtx.createBufferSource();
    src.buffer = buf;
    src.connect(this._audioCtx.destination);
    src.onended = function() { self._drainPlayQueue(); };
    this._activeSource = src;
    src.start();
  };

  VoiceClient.prototype._pollDrainComplete = function() {
    var self = this;
    var check = setInterval(function() {
      if (!self._playing) {
        clearInterval(check);
        if (!self._ttsPlaying) {
          self._setState('connected');
        }
      }
    }, 100);
  };

  // --- Mic Audio Processing + Barge-in ---

  VoiceClient.prototype._handleAudioProcess = function(e) {
    if (this._state === 'idle' || this._muted) return;

    var float32 = e.inputBuffer.getChannelData(0);

    // Send PCM16 to server natively — no client-side TTS muting so the server's
    // neural Silero VAD handles barge-in detection continuously.
    var pcm = new Int16Array(float32.length);
    for (var i = 0; i < float32.length; i++) {
      var s = Math.max(-1, Math.min(1, float32[i]));
      pcm[i] = s < 0 ? s * 0x8000 : s * 0x7FFF;
    }
    var frame = new Uint8Array(1 + pcm.byteLength);
    frame[0] = 0x01;
    frame.set(new Uint8Array(pcm.buffer), 1);
    this._send(frame.buffer);
  };

  VoiceClient.prototype._doBargeIn = function() {
    this._bargeInCooldown = true;
    var self = this;
    setTimeout(function() { self._bargeInCooldown = false; }, this._bargeInCooldownMs);

    this._onSystemMessage('Interrupting...');
    this.stopPlayback();

    // Send 0x40 Interrupt to server
    var frame = new Uint8Array([0x40]);
    this._send(frame.buffer);

    this._setState('connected');
  };

  // --- State ---

  VoiceClient.prototype._setState = function(state) {
    if (this._state === state) return;
    this._state = state;
    this._onStateChange(state);
  };

  // --- WS Send ---

  VoiceClient.prototype._send = function(buf) {
    if (this._sendBinary) {
      this._sendBinary(buf);
    } else if (this._ws && this._ws.readyState === WebSocket.OPEN) {
      this._ws.send(buf);
    }
  };

  /**
   * Update the WS reference (e.g. after reconnect).
   */
  VoiceClient.prototype.setWs = function(ws) {
    this._ws = ws;
  };

  VoiceClient.prototype.setSendBinary = function(fn) {
    this._sendBinary = fn;
  };

  return VoiceClient;
})();
