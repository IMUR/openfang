---

## title: "Reactor `rtr` Cluster Reference"

date: 2026-04-01
verified: 2026-04-07T23:00:00-07:00

# Reactor `rtr` Cluster Reference

Reality-checked cluster topology. Verified against live `ss -tlnp` and `/etc/caddy/Caddyfile` on 2026-04-07.

## Nodes


| Node       | Hostname     | User   | Tailscale IP | LAN IP            | Arch   | OS                 | Role                                 |
| ---------- | ------------ | ------ | ------------ | ----------------- | ------ | ------------------ | ------------------------------------ |
| Projector  | `projector`  | `prtr` | `100.64.0.7` | `192.168.254.172` | x86_64 | Debian 13          | Compute, AI inference, OpenClaw host |
| Cooperator | `cooperator` | `crtr` | `100.64.0.1` | `192.168.254.10`  | arm64  | RPi OS (Debian 13) | DNS, VPN, Caddy gateway, cluster ops |
| Director   | `director`   | `drtr` | `100.64.0.2` | `192.168.254.124` | x86_64 | Debian 13          | GPU inference (vLLM)                 |
| Terminator | `terminator` | `trtr` | `100.64.0.8` | —                 | arm64  | macOS              | Workstation, cluster entry-point     |
| Kalicopter | `kalicopter` | `kali` | `100.64.0.6` | `192.168.254.127` | arm    | Kali 2026.1        | KVM for Projector                    |
| Iterator   | `iterator`   | `irtr` | `100.64.0.5` | —                 | —      | iOS                | iPhone                               |


## Hardware

### Projector (`prtr`)

- **CPU:** Intel Core i9-9900X, 20 threads @ 4.50 GHz
- **GPU:** 2× NVIDIA GTX 1080 + 2× NVIDIA GTX 970
- **RAM:** 125 GiB
- **Disk:** 888 GiB root

### Cooperator (`crtr`)

- **CPU:** BCM2712, 4 cores @ 2.40 GHz (Raspberry Pi 5)
- **RAM:** 16 GiB
- **Disk:** 916 GiB root + 955 GiB fortress + 228 GiB ops (`/mnt/ops`)
- **Accelerator:** AI HAT+ 2 (Hailo-10H)

### Director (`drtr`)

- **CPU:** Intel Core i9-9900K, 16 threads @ 5.00 GHz
- **GPU:** NVIDIA RTX 2080 Mobile
- **RAM:** 63 GiB
- **Disk:** 888 GiB root

### Terminator (`trtr`)

- **CPU:** Apple M4, 10 cores
- **GPU:** M4 integrated, 10 cores
- **RAM:** 24 GiB
- **Disk:** 460 GiB APFS

## Port Block Model


| Block  | Range  | Category | Use                                          |
| ------ | ------ | -------- | -------------------------------------------- |
| Daemon | `44`** | Engine   | Service daemons, gateways, protocol surfaces |
| WebUI  | `55`** | Access   | Browser-facing UIs, consoles, panels         |
| Data   | `66**` | Storage  | Stores, caches, queues, indexes              |
| AI     | `77**` | Thought  | LLM, STT, TTS, embeddings, model serving     |


---

## Projector (`prtr`) — Verified 2026-04-07


| Port   | Bind        | Block  | Service                | Unit                       | Status |
| ------ | ----------- | ------ | ---------------------- | -------------------------- | ------ |
| `4444` | `0.0.0.0`   | `44**` | OpenClaw gateway       | `openclaw-gateway.service` | ✅ Live |
| `4477` | `0.0.0.0`   | `44**` | OpenFang agent runtime | `openfang`                 | ✅ Live |
| `5511` | `127.0.0.1` | `66**` | XTDB v2.1.0 (pgwire)   | `xtdb.service`             | ✅ Live |
| `6379` | `127.0.0.1` | `66**` | Redis                  | `redis-server`             | ✅ Live |
| `7700` | `0.0.0.0`   | `77**` | VICE voice pipeline    | `vice.service`             | ✅ Live |
| `7711` | `*`         | `77**` | Ollama (GPU #2)        | `ollama.service`           | ✅ Live |
| `9090` | `*`         | `55**` | Cockpit                | `cockpit.socket`           | ✅ Live |


**Retired (2026-04-07):** Whisper STT (7733), Kokoro TTS (7744), TTS shim (7745), LFM2.5-Audio (7722) — all services stopped and disabled. Voice pipeline moved to drtr. Port 4446 (OpenClaw internal) no longer listening.

---

## Cooperator (`crtr`) — Verified 2026-04-07

### Native Services


| Port   | Bind        | Service         | Unit                   | Status |
| ------ | ----------- | --------------- | ---------------------- | ------ |
| `1883` | `0.0.0.0`   | Mosquitto MQTT  | `mosquitto.service`    | ✅ Live |
| `3010` | `*`         | Suggestion Box  | `bun`                  | ✅ Live |
| `4422` | `127.0.0.1` | Headscale       | `headscale.service`    | ✅ Live |
| `4466` | `*`         | Forgejo web/API | `forgejo.service`      | ✅ Live |
| `4488` | `*`         | SRH (Next.js)   | `next-server`          | ✅ Live |
| `5588` | `127.0.0.1` | SearXNG         | —                      | ✅ Live |
| `6379` | `127.0.0.1` | Redis           | `redis-server.service` | ✅ Live |
| `6666` | `*`         | Forgejo SSH/git | `forgejo.service`      | ✅ Live |
| `8556` | `*`         | MediaMTX        | `mediamtx`             | ✅ Live |
| `8811` | `0.0.0.0`   | Atuin server    | `atuin`                | ✅ Live |
| `9090` | `*`         | Cockpit         | `cockpit.socket`       | ✅ Live |


**Retired/inactive:** signal-cli (7583), Hailo-Ollama (8000) — not listening as of 2026-04-07.

### Docker-Backed Services


| Port    | Bind        | Service            | Container           | Caddy Domain | Status |
| ------- | ----------- | ------------------ | ------------------- | ------------ | ------ |
| `3002`  | `0.0.0.0`   | Homepage           | `homepage`          | `www.ism.la` | ✅ Live |
| `3004`  | `127.0.0.1` | Grafana            | `grafana`           | `gfn.ism.la` | ✅ Live |
| `3100`  | `127.0.0.1` | Loki               | `loki`              | —            | ✅ Live |
| `5010`  | `0.0.0.0`   | eMCP Tool Selector | `emcp-manager`      | `mcp.ism.la` | ✅ Live |
| `5432`  | `0.0.0.0`   | eMCP Postgres      | `emcp-db`           | —            | ✅ Live |
| `5522`  | `127.0.0.1` | Headplane          | `headplane`         | `vpn.ism.la` | ✅ Live |
| `5678`  | `127.0.0.1` | n8n                | `n8n`               | `n8n.ism.la` | ✅ Live |
| `8081`  | `127.0.0.1` | Infisical          | `infisical`         | `env.ism.la` | ✅ Live |
| `8082`  | `127.0.0.1` | OpenWebUI          | `openwebui`         | `cht.ism.la` | ✅ Live |
| `8085`  | `127.0.0.1` | cAdvisor           | `cadvisor`          | —            | ✅ Live |
| `8086`  | `127.0.0.1` | Termix             | `termix`            | `ssh.ism.la` | ✅ Live |
| `8090`  | `0.0.0.0`   | eMCP gateway       | `emcp-server`       | `mcp.ism.la` | ✅ Live |
| `8123`  | `0.0.0.0`   | Home Assistant     | `homeassistant`     | `hom.ism.la` | ✅ Live |
| `8888`  | `0.0.0.0`   | Jupyter            | `jupyter`           | `jpt.ism.la` | ✅ Live |
| `9000`  | `0.0.0.0`   | Portainer          | `portainer`         | `doc.ism.la` | ✅ Live |
| `9099`  | `127.0.0.1` | Prometheus         | `prometheus`        | `prm.ism.la` | ✅ Live |
| `9100`  | `127.0.0.1` | node-exporter      | `node-exporter`     | —            | ✅ Live |
| `9115`  | `127.0.0.1` | blackbox-exporter  | `blackbox-exporter` | —            | ✅ Live |
| `9222`  | `127.0.0.1` | Lightpanda         | `lightpanda`        | —            | ✅ Live |
| `9617`  | `127.0.0.1` | Pi-hole exporter   | `pihole-exporter`   | —            | ✅ Live |
| `11434` | `0.0.0.0`   | eMCP Ollama        | `emcp-ollama`       | —            | ✅ Live |


---

## Director (`drtr`) — Verified 2026-04-07


| Port   | Bind      | Service                                | Unit                     | Status |
| ------ | --------- | -------------------------------------- | ------------------------ | ------ |
| `7733` | `0.0.0.0` | Parakeet TDT 0.6B STT (fp16, RTX 2080) | `parakeet-stt.service`   | ✅ Live |
| `7744` | `0.0.0.0` | Chatterbox-Turbo TTS (350M, RTX 2080)  | `chatterbox-tts.service` | ✅ Live |
| `9001` | `0.0.0.0` | Docker service                         | `docker-proxy`           | ✅ Live |
| `9090` | `*`       | Cockpit                                | `cockpit.socket`         | ✅ Live |


**Retired (2026-04-07):** vLLM (7766), Qwen3-TTS (7744, prev) — services stopped and disabled. RTX 2080 now allocated to voice pipeline (Chatterbox 3.2GB + Parakeet 1.4GB = ~4.6GB / 8GB).

---

## Caddy Reverse Proxy Domains (crtr)

All domains served via Caddy on crtr ports 80/443 with automatic HTTPS.

### Active


| Domain                  | Target                    | Service                                                                 |
| ----------------------- | ------------------------- | ----------------------------------------------------------------------- |
| `ism.la`                | → `www.ism.la`            | Redirect                                                                |
| `www.ism.la`            | `localhost:3002`          | Homepage (Tailscale only)                                               |
| `git.ism.la`            | `localhost:4466`          | Forgejo                                                                 |
| `sch.ism.la`            | `localhost:5588`          | SearXNG                                                                 |
| `cht.ism.la`            | `localhost:8082`          | OpenWebUI                                                               |
| `n8n.ism.la`            | `localhost:5678`          | n8n                                                                     |
| `env.ism.la`            | `localhost:8081`          | Infisical                                                               |
| `mng.ism.la`            | `localhost:9090`          | Cockpit                                                                 |
| `doc.ism.la`            | `localhost:9000`          | Portainer                                                               |
| `cam.ism.la`            | `localhost:5003`          | Frigate (Tailscale only)                                                |
| `hom.ism.la`            | `localhost:8123`          | Home Assistant                                                          |
| `jpt.ism.la`            | `localhost:8888`          | Jupyter                                                                 |
| `gfn.ism.la`            | `localhost:3004`          | Grafana                                                                 |
| `prm.ism.la`            | `localhost:9099`          | Prometheus                                                              |
| `dns.ism.la`            | `localhost:8080`          | Pi-hole                                                                 |
| `vpn.ism.la`            | `127.0.0.1:5522`          | Headplane                                                               |
| `vpn.rtr.dev`           | `localhost:4422`          | Headscale                                                               |
| `ssh.ism.la`            | `localhost:8086`          | Termix (VPN only)                                                       |
| `kvm.ism.la`            | mixed                     | KVM UI + MediaMTX (Tailscale only)                                      |
| `ace.ism.la`            | `100.64.0.7:4444`         | OpenClaw (prtr via Tailscale)                                           |
| `guy.ism.la`            | `100.64.0.7:4477`         | OpenFang agent runtime (prtr via Tailscale)                             |
| `vox.ism.la`            | placeholder (503)         | **Unused** (voice is integrated into `guy.ism.la`)                      |
| `tts.ism.la`            | `100.64.0.7:7745`         | TTS shim — ⚠️ stale, service disabled on prtr                           |
| `btr.ism.la`            | `192.168.254.123:80`      | Barter                                                                  |
| `srh.ism.la`            | `localhost:4488`          | SRH (Next.js)                                                           |
| `sys.ism.la`            | file server               | Cluster profiles (static)                                               |
| `mcp.ism.la`            | `localhost:5010` / `8090` | eMCP (path-based routing)                                               |
| `aboxofsuggestions.com` | `localhost:3010`          | Suggestion Box                                                          |


### Placeholder / 503


| Domain       | Notes                                                         |
| ------------ | ------------------------------------------------------------- |
| `acn.ism.la` | Archon — not deployed                                         |
| `api.ism.la` | Archon API — not deployed                                     |
| `bot.ism.la` | Bot — not deployed                                            |
| `cfg.ism.la` | Config UI — not deployed                                      |
| `thm.ism.la` | THM theme adapter — not deployed                              |
| `dot.ism.la` | DotDash console — not deployed                                |
| `dtb.ism.la` | PottySnitch PocketBase — target `localhost:8091`, no listener |


## Dev Environment

- **Runtime:** `bun`, `python 3.14`, `uv`, `node 24` (via `mise`)
- **Dotfiles:** `chezmoi` + `mise`
- **SSH priority:** crtr → drtr → trtr → prtr → kali

