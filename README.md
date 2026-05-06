# Lucidos

Lucidos is an AI companion that shares your whole workspace. You talk, build, run, automate, and remember together - in one place. Chat lives alongside your work in a persistent split view on desktop, and as a separate pane you can swipe to on mobile. You're never leaving your work to talk to the AI; you're always working with it - manifesting your intent in a rich, extensible interface.

> *If You Can Describe It, It Exists*

A local, event-driven, AI-native operating system. You own and store all the data yourself. The prompt is the primary interface, but Lucidos is a unified, integrated environment where configurable LLMs know all about your apps, triggers (scheduled or event-driven), and files. Describe an app and it's running in seconds - no build step, no deploy. Built to augment the *user*, it remembers everything you do. While it is not an "autonomous" role playing entity, you can automate anything. Nothing happens without a user intent, though - which is a central concept for Lucidos.

The integrated environment makes for a smooth user experience, where researching a topic, storing the findings, spinning up an app around them, and scheduling regular syncs with external data sources can all be done in one simple flow. Local data lives in a Postgres event store or as git-versioned artifacts.

---

## Prerequisites

- **Rust** (stable toolchain)
- **Docker** (for PostgreSQL + pgvector)
- **Node.js** (for Vite frontend dev server)
- **Vertex AI access** or **OpenAI API key** (for the LLM)

## Dev Setup

```bash
# Start with a workspace directory
./scripts/web-dev.sh -w ~/workspaces/personal

# Build engine first if no binary exists
./scripts/web-dev.sh -w ~/workspaces/personal -b

# Other scripts
./scripts/stop.sh              # Graceful shutdown
./scripts/restart.sh -w <ws>   # Stop and start
./scripts/status.sh            # Check health
```

This starts PostgreSQL in Docker, builds/runs the Rust engine natively, and launches a Vite dev server. Each workspace gets its own ports — multiple workspaces can run concurrently.

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `LUCIDOS_WORKSPACE` | — | Workspace directory |
| `LUCIDOS_MODEL` | `claude-opus-4-7` | LLM model name |
| `VERTEX_PROJECT_ID` | — | GCP project (for `claude-*`/`gemini-*`) |
| `VERTEX_REGION` | `europe-west1` | Vertex AI region |
| `OPENAI_API_KEY` | — | OpenAI key (for `gpt-*` models) |

---

## HTTPS for Local Development

Lucidos uses [mkcert](https://github.com/FiloSottile/mkcert) to generate locally-trusted TLS certificates. When certs are present in `.certs/`, Vite automatically serves over HTTPS.

### Install mkcert and generate certs

```bash
brew install mkcert
mkcert -install            # adds the CA to macOS trust store + Chrome/Firefox
mkdir -p .certs
mkcert -cert-file .certs/cert.pem -key-file .certs/key.pem \
  localhost 127.0.0.1 ::1 \
  "$(ipconfig getifaddr en0)" \
  "$(tailscale status --self --json | python3 -c "import sys,json; print(json.load(sys.stdin)['Self'].get('DNSName','').rstrip('.'))")"
```

Add your local IP and Tailscale hostname so the cert is valid when accessed from other devices. If your IP changes, regenerate the cert.

### Accessing from iPhone / iPad over Tailscale

The mkcert root CA must be trusted on iOS for HTTPS to work:

1. **Find the CA cert:**
   ```bash
   mkcert -CAROOT   # prints the directory, e.g. ~/Library/Application Support/mkcert
   ```
2. **Transfer `rootCA.pem` to your device** — AirDrop is easiest.
3. **Install the profile:** Open the file on iOS. Go to **Settings > General > VPN & Device Management** and install the downloaded profile.
4. **Enable full trust:** Go to **Settings > General > About > Certificate Trust Settings** and toggle on the mkcert root CA.

After this, Safari and Chrome on iOS will trust your dev server's HTTPS certificate.

### Notes

- **`.certs/` is gitignored** — certs are machine-specific, never committed.
- **HTTP still works.** If `.certs/cert.pem` doesn't exist, Vite falls back to plain HTTP automatically. HTTPS is opt-in.
- **Restart Chrome** after running `mkcert -install` for the first time — Chrome caches the CA store and won't pick up the new root CA until restarted.
- **Service workers require HTTPS** (or localhost). If testing push notifications or PWA features from a mobile device over Tailscale, HTTPS is required.

---

## Architecture

```
┌──────────────────┐     ┌─────────────────────────┐
│   Tauri App      │────▶│   Docker Container      │
│   (Desktop UI)   │◀────│   (Workspace + Engine)  │
└──────────────────┘     └─────────────────────────┘
```

### Tech Stack

| Component | Choice |
|-----------|--------|
| Language | Rust |
| UI | Tauri (Rust + web frontend) |
| LLM | Vertex AI / OpenAI (configurable) |
| Event Store | PostgreSQL + pgvector |
| Embeddings | fastembed (in-process, local) |
| Execution | Lucidos-managed Python |
| Packaging | Docker + Tauri desktop app |

### Workspace Structure

```
/workspace/
  .lucidos/            # Ephemeral runtime (deletable)
  data/
    artifacts/        # Git-tracked user files
    skills/           # Skill definitions + UIs
    postgres/         # Event store (gitignored)
```

### Core Concepts

- **Events** — Immutable, append-only records of confirmed outcomes. The single source of truth.
- **Artifacts** — Versioned outputs (code, documents, apps) stored in Git. Addressable, with provenance.
- **Skills** — Interactive plugins combining LLM instructions with optional web UIs.
- **Prompt-first UI** — Everything doable in apps must be doable via the prompt.

### Key Invariants

- Events are immutable
- Git commits are consequences, not causes
- No hidden state or silent decisions
- Replay reconstructs truth, not creativity

---

## Versioning

Two independent version axes:

- **`RELEASE`** (repo root) — the umbrella user-facing version of the Lucidos
  release that bundles all crates. Currently `0.7`. Think Ubuntu 24.04: one
  number per shipped release, regardless of how the individual components
  inside have moved. Exposed at runtime as `lucidos_engine::LUCIDOS_RELEASE`,
  in the `release` field of `/api/health`, in the engine's `--version` output,
  and in the desktop app's control panel.
- **Per-crate `Cargo.toml` versions** — semver per component (lucidos-engine,
  lucidos-app, etc.), bumped on their own cadence by `build.rs`. Visible as
  `engine_version` / `latest_tauri_app_version` in `/api/health`.

### Cutting a Lucidos release

Releases are produced as squashed-orphan commits on `lucidos/main` (one per
`v<version>` tag); per-release notes live in `CHANGELOG.md`.
