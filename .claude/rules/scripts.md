---
globs:
  - "scripts/**"
  - "**/*.sh"
  - "Makefile"
---

# Scripts & Build

## Dev / runtime scripts

```bash
./scripts/web-dev.sh -w <ws> [-b] [-r]    # Start (-b builds; -r release)
./scripts/tauri-dev.sh -w <ws> [-b]       # Start engine + Tauri window
./scripts/stop.sh -w <ws>                 # Stop a specific workspace
./scripts/status.sh                       # Check running status
./scripts/populate.sh -w <ws> [-c]        # Populate test history
./scripts/new-migration.sh <description>  # Create timestamped migration
./scripts/dev-codesign-setup.sh           # One-time: stable macOS code-signing identity
./scripts/test-engine.sh [--full|--fresh] # Engine tests against a dedicated Docker PG
```

## Engine tests need Postgres — use `test-engine.sh`

The engine's integration tests (`setup_test_db` in `crates/lucidos-engine/src/test_support.rs`) need a **real Postgres**: each test `CREATE`s a throwaway `lucidos_test_*` database, runs migrations, and drops it. The connection comes from `TEST_DATABASE_URL`, falling back to a hardcoded `localhost:5432`. **Running bare `cargo test -p lucidos-engine` with no `TEST_DATABASE_URL` and no PG up makes every DB-backed test panic on connect** (`.expect("admin connect")`) — that's hundreds of false "failures", not regressions.

```bash
make test                       # → ./scripts/test-engine.sh  (cargo test --lib)
make test-full                  # → ./scripts/test-engine.sh --full  (whole crate)
./scripts/test-engine.sh -- -- migration_tests   # pass filters through to cargo test
./scripts/test-engine.sh --fresh                 # recreate the test DB container clean
```

`test-engine.sh` provisions a **dedicated, disposable** `lucidos-pg-test` container (`pgvector/pgvector:pg17`, port `LUCIDOS_TEST_PG_PORT` / default `5510`), exports `TEST_DATABASE_URL`, then runs cargo test. It is isolated from every workspace's PG (separate name + port) so a test run can't mutate `~/workspaces/*` data, and it **never broad-kills** — it touches only its own container by exact name (the prior `test-engine.sh` was deleted for `pkill -f cognos-engine`). To run cargo directly instead, start the container once and `export TEST_DATABASE_URL` yourself.

Always use `web-dev.sh -b` to restart. `scripts/lib/ports.sh` allocates per-workspace ports; engine reverse-proxies to Vite. Postgres containers (`lucidos-pg-<cksum>`) stay running when engine stops.

### macOS code signing (stable TCC grants)

A `cargo build` engine binary is `adhoc, linker-signed`; its CDHash changes every rebuild, so macOS TCC (privacy) discards prior permission grants and re-prompts ("lucidos-engine would like to access …") after each rebuild. `build_or_find_engine` (in `scripts/lib/workspace.sh`) re-signs the freshly built binary with a **stable self-signed identity** (`scripts/lib/codesign.sh` → `sign_engine_binary`), giving it a rebuild-stable Designated Requirement so one Allow click persists. Run `./scripts/dev-codesign-setup.sh` **once** first — it creates + trusts the cert (single GUI password prompt). Until then signing is a no-op and the build proceeds unsigned (with a hint). This only stops the re-prompting; the prompt still names "lucidos-engine" (a post-fork TCC responsibility disclaim to attribute it to Claude Code is not possible — see the note in `runtime/claude_code.rs::build_command`).

**Search-list registration is load-bearing.** `codesign --sign <name>` resolves the identity through the **keychain search list**, not the `--keychain` flag — so the dedicated `lucidos-dev-signing.keychain-db` must be *in the search list* or every sign fails with "no identity found" and silently falls back to ad-hoc (the prompts never stop, even though `find-identity -p codesigning "$KEYCHAIN"` reports the identity as valid). `lucidos_ensure_keychain_in_search_list` (in `codesign.sh`) registers it; both setup and `sign_engine_binary` call it, so existing installs self-heal on the next `-b` build. **Per-binary, not per-workspace:** every engine binary signed with the same identifier (`lucidos-engine`) + same cert leaf shares one Designated Requirement, so a single Allow covers all workspaces. But a binary built outside the scripts — e.g. `cargo run` from an IDE — bypasses `sign_engine_binary` and stays ad-hoc; launch via `web-dev.sh` so it gets signed.

## Build

```bash
cargo build -p lucidos-engine --release    # Engine
cd crates/lucidos-app && cargo tauri build # Desktop app
```

Dev: native engine + Docker PostgreSQL. Production: single Docker container. Makefile: `make build`, `make test`, `make run`.
