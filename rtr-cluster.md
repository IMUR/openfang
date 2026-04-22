---

## title: "Reactor `rtr` Cluster Reference"

date: 2026-04-01
verified: 2026-04-22

# Reactor `rtr` Cluster Reference

Reality-checked cluster topology. Verified against live `ss -tlnp`, `ip addr show`, and Caddyfile on 2026-04-11.

## Nodes


| Node       | Hostname     | User   | Tailscale IP | LAN IP            | Arch   | OS                 | Role                                 |
| ---------- | ------------ | ------ | ------------ | ----------------- | ------ | ------------------ | ------------------------------------ |
| Projector  | `projector`  | `prtr` | `100.64.0.7` | `192.168.254.22`  | x86_64 | Debian 13          | Compute, AI inference, OpenClaw host |
| Cooperator | `cooperator` | `crtr` | `100.64.0.1` | `192.168.254.11`  | arm64  | RPi OS (Debian 13) | DNS, VPN, Caddy gateway, cluster ops |
| Director   | `director`   | `drtr` | `100.64.0.2` | `192.168.254.33`  | x86_64 | Debian 13          | GPU inference, voice pipeline        |
| Terminator | `terminator` | `trtr` | `100.64.0.8` | —                 | arm64  | macOS              | Workstation, cluster entry-point     |
| Kalicopter | `kalicopter` | `kali` | `100.64.0.6` | `192.168.254.127` | arm    | Kali 2026.1        | KVM for Projector                    |
| Iterator   | `iterator`   | `irtr` | `100.64.0.5` | —                 | —      | iOS                | iPhone                               |
| Zrtr       | `zrtr`       | `zrtr` | `100.64.0.3` | —                 | —      | —                  | —                                    |


**Gateway vIP:** `192.168.254.10/24` is a VRRP virtual IP managed by keepalived. `crtr` holds it at priority 150 (MASTER); `prtr` takes over at priority 100 (BACKUP) if `crtr` is unreachable.

## Hardware

### Projector (`prtr`)

- **CPU:** Intel Core i9-9900X, 20 threads @ 4.50 GHz
- **GPU:** 2× NVIDIA GTX 1080 + 2× NVIDIA GTX 970
- **RAM:** 125 GiB
- **Disk:** 888 GiB root

### Cooperator (`crtr`)

- **CPU:** BCM2712, 4 cores @ 2.40 GHz (Raspberry Pi 5)
- **RAM:** 16 GiB
- **Disk:** 916 GiB root (hosts 128 GiB `/var/lib/ops.img` loopback backing `/mnt/ops`)
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
| Data   | `66`** | Storage  | Stores, caches, queues, indexes              |
| AI     | `77`** | Thought  | LLM, STT, TTS, embeddings, model serving     |


---

## Projector (`prtr`) — Verified 2026-04-11

| Port   | Bind        | Block  | Service                      | Unit                       | Status                                             |
| ------ | ----------- | ------ | ---------------------------- | -------------------------- | -------------------------------------------------- |
| `4444` | `0.0.0.0`   | `44`** | OpenClaw gateway       | `openclaw-gateway.service` | ✅ Live |
| `4477` | `0.0.0.0`   | `44`** | OpenFang agent runtime | `openfang`                 | ✅ Live |
| `5511` | `127.0.0.1` | `66`** | XTDB v2.1.0 (pgwire)   | `xtdb.service`             | ✅ Live |
| `6379` | `127.0.0.1` | `66**` | Redis                  | `redis-server`             | ✅ Live |
| `7711` | `*`         | `77**` | Ollama (GPU #1 + #2)   | `ollama.service`           | ✅ Live |
| `9090` | `*`         | `55**` | Cockpit                | `cockpit.socket`           | ✅ Live |

---

## Cooperator (`crtr`) — Verified 2026-04-11

### Native Services

| Port   | Bind        | Service                | Unit                   | Status |
| ------ | ----------- | ---------------------- | ---------------------- | ------ |
| `53`   | `0.0.0.0`   | DNS (Pi-hole)          | `pihole-FTL`           | ✅ Live |
| `1883` | `0.0.0.0`   | Mosquitto MQTT         | `mosquitto.service`    | ✅ Live |
| `3010` | `*`         | Suggestion Box         | `bun`                  | ✅ Live |
| `4400` | `0.0.0.0`   | KVM Console (IPMI)     | `kvm-console.service`  | ✅ Live |
| `4422` | `127.0.0.1` | Headscale              | `headscale.service`    | ✅ Live |
| `4466` | `*`         | Forgejo web/API        | `forgejo.service`      | ✅ Live |
| `4488` | `*`         | SRH (Next.js)          | `next-server`          | ✅ Live |
| `5588` | `127.0.0.1` | SearXNG                | —                      | ✅ Live |
| `6379` | `127.0.0.1` | Redis                  | `redis-server.service` | ✅ Live |
| `6666` | `*`         | Forgejo SSH/git        | `forgejo.service`      | ✅ Live |
| `7788` | `0.0.0.0`   | hailo-ollama (Hailo-10H) | `hailo-ollama.service` | ✅ Live |
| `8080` | `0.0.0.0`   | Pi-hole web UI         | `pihole-FTL`           | ✅ Live |
| `8811` | `0.0.0.0`   | Atuin server           | `atuin`                | ✅ Live |
| `9090` | `*`         | Cockpit                | `cockpit.socket`       | ✅ Live |

**Retired:** `8000` (Hailo-Ollama — moved to :7788) — confirmed not listening.

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

## Director (`drtr`) — Verified 2026-04-11

| Port   | Bind      | Service                                | Unit                     | Status |
| ------ | --------- | -------------------------------------- | ------------------------ | ------ |
| `7733` | `0.0.0.0` | Parakeet TDT 0.6B STT (fp16, RTX 2080) | `parakeet-stt.service`   | ✅ Live |
| `7744` | `0.0.0.0` | Chatterbox-Turbo TTS (RTX 2080)        | `chatterbox-tts.service` | ✅ Live |
| `9001` | `0.0.0.0` | Docker service                         | `docker-proxy`           | ✅ Live |
| `9090` | `*`       | Cockpit                                | `cockpit.socket`         | ✅ Live |

**RTX 2080 allocation:** Chatterbox ~3.2 GB + Parakeet ~1.4 GB ≈ 4.6 GB / 8 GB.
**Retired:** vLLM (7766) — stopped.

---

## Caddy Reverse Proxy Domains (crtr)

All domains served via Caddy on crtr ports 80/443 with automatic HTTPS.

### Active

| Domain                  | Target                                                                                  | Service                                     |
| ----------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------- |
| `ism.la`                | → `www.ism.la`                                                                          | Redirect                                    |
| `www.ism.la`            | `localhost:3002`                                                                        | Homepage (Tailscale only)                   |
| `git.ism.la`            | `localhost:4466`                                                                        | Forgejo                                     |
| `sch.ism.la`            | `localhost:5588`                                                                        | SearXNG                                     |
| `cht.ism.la`            | `localhost:8082`                                                                        | OpenWebUI                                   |
| `n8n.ism.la`            | `localhost:5678`                                                                        | n8n                                         |
| `env.ism.la`            | `localhost:8081`                                                                        | Infisical                                   |
| `mng.ism.la`            | `localhost:9090`                                                                        | Cockpit                                     |
| `doc.ism.la`            | `localhost:9000`                                                                        | Portainer                                   |
| `cam.ism.la`            | `localhost:5003`                                                                        | Frigate (Tailscale only)                    |
| `hom.ism.la`            | `localhost:8123`                                                                        | Home Assistant                              |
| `jpt.ism.la`            | `localhost:8888`                                                                        | Jupyter                                     |
| `gfn.ism.la`            | `localhost:3004`                                                                        | Grafana                                     |
| `prm.ism.la`            | `localhost:9099`                                                                        | Prometheus                                  |
| `dns.ism.la`            | `localhost:8080`                                                                        | Pi-hole                                     |
| `vpn.ism.la`            | `127.0.0.1:5522`                                                                        | Headplane                                   |
| `vpn.rtr.dev`           | `localhost:4422`                                                                        | Headscale                                   |
| `ssh.ism.la`            | `localhost:8086`                                                                        | Termix (VPN only)                           |
| `kvm.ism.la`            | `localhost:4400`                                                                        | KVM Console (Tailscale only)                |
| `ace.ism.la`            | `100.64.0.7:4444`                                                                       | OpenClaw (prtr via Tailscale)               |
| `guy.ism.la`            | `100.64.0.7:4477`                                                                       | OpenFang agent runtime (prtr via Tailscale) |
| `btr.ism.la`            | `192.168.254.123:80`                                                                    | Barter                                      |
| `srh.ism.la`            | `localhost:4488`                                                                        | SRH (Next.js)                               |
| `mcp.ism.la`            | `localhost:5010` / `8090`                                                               | eMCP (path-based routing)                   |
| `aboxofsuggestions.com` | `localhost:3010`                                                                        | Suggestion Box                              |

### Placeholder / 503

| Domain       | Notes                                                         |
| ------------ | ------------------------------------------------------------- |
| `vox.ism.la` | Voice UI — retired, voice is in the OpenFang dashboard        |
| `acn.ism.la` | Archon — not deployed                                         |
| `api.ism.la` | Archon API — not deployed                                     |
| `bot.ism.la` | Bot — not deployed                                            |
| `cfg.ism.la` | Config UI — not deployed                                      |
| `thm.ism.la` | THM theme adapter — not deployed                              |
| `sys.ism.la` | Cluster profiles — retired                                    |
| `dot.ism.la` | DotDash console — not deployed                                |
| `dtb.ism.la` | PottySnitch PocketBase — target `localhost:8091`, no listener |

---

## Shared Storage — Verified 2026-04-21

Cluster-wide read/write store. Served from crtr via Samba, backed by a local ext4 loopback image on crtr's root filesystem.

### Server

| Share              | Path on crtr | Backing                                               | Allocated | Used  |
| ------------------ | ------------ | ----------------------------------------------------- | --------- | ----- |
| `//100.64.0.1/ops` | `/mnt/ops`   | `/var/lib/ops.img` (ext4 loopback, label `ops-local`) | 128 GiB   | 21 GB |

- `smb.conf` stanzas: `[global]`, `[ops]`.
- VFS modules on `[ops]`: `fruit streams_xattr` (macOS-native metadata).
- Access: `valid users = @cluster`, `force group = cluster`, 0775 masks.
- fstab: `/var/lib/ops.img /mnt/ops ext4 loop,noatime,nofail 0 2`

### Client mounts

| Node | Mechanism                                          | Mount point    |
| ---- | -------------------------------------------------- | -------------- |
| drtr | fstab CIFS with `x-systemd.automount` (vers=3.1.1) | `/mnt/ops`     |
| prtr | fstab CIFS with `x-systemd.automount` (vers=3.1.1) | `/mnt/ops`     |
| trtr | Finder favorite `smb://cooperator/ops`             | `/Volumes/ops` |

Credentials on drtr/prtr: `/root/.smbcredentials` (0600).

### Conventions

- `/mnt/ops/records/<node>-records/` — session records (see `record-keeping` skill).
- `/mnt/ops/prj/` — cross-node project source. Default ACLs preserve `user:crtr:rwx`, `group:cluster:rwx` inheritance on new files.
- `/mnt/ops/docker/` — compose files and persistent volumes for crtr-hosted docker stacks.

---

## SSH Access

Config is chezmoi-managed and identical on all cluster nodes. Any node can reach any other by alias:

```bash
ssh <alias>   # e.g. ssh drtr, ssh crtr, ssh kali
```

| Alias  | Tailscale IP | User   | Key                      | Notes                      |
| ------ | ------------ | ------ | ------------------------ | -------------------------- |
| `crtr` | `100.64.0.1` | `crtr` | `~/.ssh/id_ed25519`      |                            |
| `drtr` | `100.64.0.2` | `drtr` | `~/.ssh/id_ed25519`      |                            |
| `zrtr` | `100.64.0.3` | `zrtr` | `~/.ssh/id_ed25519`      |                            |
| `kali` | `100.64.0.6` | `kali` | `~/.ssh/id_ed25519`      |                            |
| `prtr` | `100.64.0.7` | `prtr` | `~/.ssh/id_ed25519_self` | Self-SSH uses separate key |
| `trtr` | `100.64.0.8` | `trtr` | `~/.ssh/id_ed25519`      |                            |

**Forgejo SSH** uses a non-standard port:

```bash
git clone ssh://git@git.ism.la:6666/<org>/<repo>.git
```

Connection multiplexing is pre-configured (`ControlMaster auto`, `ControlPersist 10m`). Sockets land in `~/.ssh/sockets/`. Keep-alive is active (`ServerAliveInterval 60`, `ServerAliveCountMax 3`).

**RouterOS gateway:**

```bash
ssh admin@192.168.254.254
```

## Toolchain

All runtime versions are pinned and managed by `mise`; dotfiles by `chezmoi`. See the `colab-ops` skill for the full inventory, ownership rules, and change workflow.

- **Python:** Use `uv` for package installation (for example, `uv add` in a project or `uv tool install` for global tools) and, when isolation is needed, for virtual environments (`uv venv`). Prefer `uvx` for one-off tool invocations.

| Node   | Version | `python3` resolves to                                                 | Via           |
| ------ | ------- | --------------------------------------------------------------------- | ------------- |
| `prtr` | 3.14.3  | `~/.local/share/mise/installs/python/3.14.3/bin/python3`              | mise (direct) |
| `crtr` | 3.14.3  | `~/.local/share/mise/shims/python3`                                   | mise shim     |
| `drtr` | 3.14.3  | `~/.local/share/mise/shims/python3`                                   | mise shim     |
| `trtr` | 3.14.3  | `/opt/homebrew/bin/python3` ⚠️ (shadowing mise — `python` is correct) | Homebrew      |

- **JavaScript:** `bun` + `node 24` via `mise`. Use `bun run` / `bunx` in place of `npm run` / `npx`.
- **Dotfiles / versions:** `chezmoi update --force` to apply; `mise install --yes` after version bumps.

### Shell Environment Warning

`chezmoi` owns `.zshrc`, `.profile`, `.bashrc`, and `.zshenv` on every node. Direct edits to those files **will be clobbered** on the next `chezmoi update --force`.

Node-specific overrides belong in the local files, which chezmoi never touches and which are sourced automatically at the end of the managed configs:

| File               | Sourced by | Present on                     |
| ------------------ | ---------- | ------------------------------ |
| `~/.zshrc.local`   | `.zshrc`   | `prtr`, `drtr`, `trtr`         |
| `~/.profile.local` | `.profile` | `prtr`, `crtr`, `drtr`, `trtr` |

In practice these hold things like host-specific env vars (`OLLAMA_HOST`), headless auth token overrides, platform SDK paths, shell helpers, and tool completions that vary per node. If you need to persist something node-specific, it goes here — not in the chezmoi-managed files.
