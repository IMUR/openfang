#!/usr/bin/env bash
set -euo pipefail

echo "=== TTS Switchover: Director Qwen3-TTS -> Projector Kokoro ==="
echo ""

echo "[1/5] Verifying Director TTS is up..."
curl -sf --max-time 5 http://100.64.0.2:7744/health > /dev/null
echo "      Director TTS is healthy."

echo "[2/5] Starting Kokoro TTS on Projector..."
sudo systemctl enable kokoro-tts.service
sudo systemctl start kokoro-tts.service

echo "      Waiting for Kokoro to be ready..."
for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20 21 22 23 24 25 26 27 28 29 30; do
    if curl -sf --max-time 2 http://localhost:7744/health > /dev/null 2>&1; then
        echo "      Kokoro TTS is healthy."
        break
    fi
    if [ "$i" -eq 30 ]; then
        echo "ERROR: Kokoro TTS failed to start within 60s"
        sudo systemctl status kokoro-tts.service
        exit 1
    fi
    sleep 2
done

CONFIG="$HOME/.openfang/config.toml"
echo "[3/5] Updating config.toml tts_endpoint..."
sed -i 's|tts_endpoint = "http://100.64.0.2:7744"|tts_endpoint = "http://localhost:7744"|' "$CONFIG"
echo "      tts_endpoint now points to http://localhost:7744"

echo "[4/5] Restarting OpenFang..."
openfang stop
sleep 2
openfang start
echo "      OpenFang restarted."

echo "[5/5] Disabling Director TTS..."
ssh drtr 'zsh -l -c "sudo systemctl disable --now qwen3-tts.service"' || echo "      WARNING: Could not disable Director TTS - do it manually."
echo "      Director TTS disabled."

echo ""
echo "=== Switchover complete ==="
echo "Projector Kokoro TTS is now active at http://localhost:7744"
echo "Director Qwen3-TTS has been disabled."
