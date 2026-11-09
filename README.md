# Airlink Daemon

**Node agent for Airlink Panel — v2.0.0-rc1-beta**

The daemon runs on each node server and handles container lifecycle, resource monitoring, file management, and SFTP. The panel communicates with it over HTTP using basic auth and optional HMAC request signing.

---

## Overview

Each physical or virtual machine that hosts game servers runs one daemon instance. The panel registers the machine as a node and sends commands to it. The daemon executes them against Docker, streams console output back over WebSocket, and exposes a file system API for the panel's file manager.

---

## Prerequisites

- Rust 1.80+ (or Cargo)
- Git
- Docker (running and accessible to the daemon process)

---

## Installation & Running (Rust)

### Step 1 — Clone the repository

```bash
cd /etc/
git clone https://github.com/AirlinkLabs/daemon.git
cd daemon
```

### Step 2 — Configure environment

```bash
cp example.env .env
```

Or configure automatically with the panel:
```bash
cargo run --bin configure -- --panel http://<panel-ip>:<panel-port> --key <key>
```

### Step 3 — Build & Run

```bash
# Debug run
cargo run --bin airlinkd

# Release build
cargo build --release
./target/release/airlinkd
```

---

## Security

### Basic auth

Every request to the daemon must include HTTP basic auth credentials with `Airlink` as the username and the node `KEY` as the password. The panel sends these automatically.

### HMAC request signing (new in 2.0.0-rc1)

The daemon now ships with `hmacMiddleware` — a second layer of request verification on top of basic auth. When enabled, the panel signs every outbound request with an HMAC-SHA256 signature derived from the request method, path, body, and a Unix timestamp. The daemon verifies the signature and rejects requests where the timestamp is more than 30 seconds old (replay protection).

**Permissive mode (default)** — requests without HMAC headers are logged and allowed through. This keeps the daemon compatible with older panel versions.

**Strict mode** — set `REQUIRE_HMAC=true` in `.env`. Requests without valid HMAC headers are rejected with `401`. Use this when both the panel and daemon are on 2.0.0-rc1 or later.

The signing algorithm:
```
payload  = "{timestamp}:{METHOD}:{path}:{body}"
signature = HMAC-SHA256(KEY, payload)
headers  = X-Airlink-Timestamp, X-Airlink-Signature
```

---

## Architecture

```
src/
├── app/
│   ├── app.ts              — Express setup and graceful shutdown
│   ├── hmacMiddleware.ts   — HMAC request signature verification (new)
│   ├── init.ts             — Startup sequence
│   ├── middleware.ts       — Basic auth, body parsing, logging
│   └── routes.ts           — Dynamic async route loader
├── handlers/
│   ├── instances/          — Container lifecycle (create, start, stop, kill, delete, attach)
│   ├── filesystem/         — File read/write/delete/rename
│   ├── minecraft/          — Player count fetching via server-status protocol
│   ├── radar/              — VirusTotal file scanning
│   ├── sftp/               — Built-in SFTP server
│   └── stats.ts            — CPU/RAM/disk metrics with per-period filtering
├── routes/
│   ├── core.ts             — Health check, system stats
│   ├── instances.ts        — Container control endpoints
│   ├── fileSystem.ts       — File manager endpoints
│   ├── minecraft.ts        — Player data endpoint
│   ├── radar.ts            — File scan endpoint
│   └── sftp.ts             — SFTP credential endpoints
└── utils/
    ├── config.ts           — Environment config loader
    ├── errorHandler.ts     — Global error handler
    ├── fileSpecifier.ts    — Safe path resolution
    ├── logger.ts           — Structured logger
    └── validation.ts       — Input validation helpers
```

---

## Links

- Panel: [github.com/airlinklabs/panel](https://github.com/airlinklabs/panel)
- Website: [airlinklabs.github.io/home](https://airlinklabs.github.io/home/)
- Docs: [airlinklabs.github.io/home/docs/quickstart](https://airlinklabs.github.io/home/docs/quickstart/)
- Discord: [discord.gg/ujXyxwwMHc](https://discord.gg/ujXyxwwMHc)

---

## License

MIT — see [`LICENSE`](LICENSE) for details.
