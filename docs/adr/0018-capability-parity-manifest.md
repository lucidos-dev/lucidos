# 0018 — Agent surfaces stay in sync via a capability parity manifest + codegen, not blanket parity

- **Status** — Accepted
- **Date** — 2026-06-26

## Context

Marking notifications read was clunky for the Lucidos agent: it took ~12 tool
calls of `curl`/port/gateway-prefix guessing because there was **no LLM tool and
no CLI command** to mark notifications read — only `read_notifications` (LLM) and
`notify`/send (CLI). The HTTP endpoints, the SDK methods (`markRead` /
`markAllRead`), and the UI all already existed.

A 4-surface audit confirmed the pattern is structural: the two **agent-facing
surfaces — LLM tools and the `lucidos` CLI — silently drift behind UI/SDK/HTTP**.
The CLI is the thinnest agent surface, which is why coding-agent subprocesses keep
falling back to raw `curl` and hit every gateway/port/protocol trap. There was no
mechanism preventing a capability from landing on one surface and not the others.

## Decision

1. Keep agent surfaces in sync via **declared parity, not blanket parity**: a
   single **capability parity manifest** (Rust, engine crate) is the source of
   truth, and each capability declares *which* surfaces it belongs to (`llm` /
   `cli` / `sdk`; `ui` / `http` are the substrate). Drift = a capability present
   in some declared surfaces but missing from others.
2. **Generate** the agent surfaces from the manifest (CLI subcommands, SDK
   request wrappers, in-crate LLM tool schemas), enforced by staleness tests + a
   per-operation handler trait, so a capability cannot exist on one surface
   without its declared siblings.
3. LLM tools are **domain-grouped with active consolidation** — one tool per
   domain with an `operation` enum — to shrink the model's tool-selection surface
   (target ~94 → ~50–60 tools) while adding the missing capabilities; hot
   single-purpose tools stay standalone.

## Rationale

- **Declared, not blanket.** ~150 HTTP routes exist; most (backup schedules, disk
  usage, device registration, blobs, compose drafts, presence, push subscribe,
  SSE, `/internal/*` hooks) are intentionally UI/infra-only. A "everything in
  every surface" rule would force nonsense tools and a strict test would fail on
  intentional asymmetries. Per-capability surface declaration is the only honest
  model.
- **Codegen over convention.** A CLAUDE.md rule + `/harden` review is not
  deterministic — it relies on the agent noticing. Generating the surfaces from
  one SSOT makes drift a *build failure*, matching the existing enforced patterns
  (`thread_lifecycle.rs` codegen, `preference_catalog.rs` sync test,
  `navigate_targets_codegen`).
- **Grouping fixes the real cost.** The cost of more tools is the model's
  selection accuracy across the list, not tokens. Consolidating scattered
  per-verb tools into grouped domain tools *reduces* that surface even as parity
  grows it.

## Consequences

- New agent capabilities are added in **one place** (the manifest); the generator
  + the per-operation handler trait force every declared surface to be wired, or
  the build fails. We keep determinism; we give up the freedom to hand-roll a
  one-off tool/CLI command outside the manifest.
- Generated files are never hand-edited (AUTO-GENERATED header + staleness test).
- Consolidated/renamed tools keep **aliases** so existing prompts/threads don't
  break.
- Contract tests prove *parity and non-drift*; they cannot prove *selection
  quality* — that stays a manual dogfood/eval responsibility.
- The CLI generator routes through the gateway-safe `http.rs` client, so generated
  commands can never re-introduce the curl/port/prefix traps that motivated this.

## Amendment, 2026-08-05: the `threads` domain's CLI is hand-written, and cannot currently be otherwise

The "we give up the freedom to hand-roll a one-off CLI command" consequence above
has one standing exception, recorded here so it is not rediscovered as an
oversight, and so nobody flips a flag expecting it to work.

`THREADS_DOMAIN.cli` is `false` and its `list` / `count` CLI arms are
hand-written. Adding a third capability to that family (`threads follow-up`,
`POST /api/v1/threads/:thread_id/follow-up`) did not add a manifest operation,
for two reasons that are properties of the generator rather than preferences:

1. **The generator filters at the domain level first.**
   `codegen.rs` iterates `domains().iter().filter(|d| d.cli)` and only then
   `.filter(|o| o.on_cli(d))`. So an operation flagged `cli: Some(true)` inside a
   `cli: false` domain generates **nothing at all**, while `on_cli` still reports
   `true`. The manifest entry would declare a surface that does not exist, which
   is worse than no entry.
2. **The domain cannot be flipped without inventing arguments.** `THREADS_OPS`
   declares `args: &[]` for both `list` and `count`; their filters
   (`active`, `source`, `limit`, and now `parent`) live only in `llm_schema`,
   which the CLI generator never reads. Flipping `THREADS_DOMAIN.cli = true`
   would therefore generate a flagless `threads list` and a `ThreadsCmd` enum
   colliding with the hand-written one, i.e. a regression plus a build break.

Folding the domain in properly means declaring those filters as real `Arg`s so
the LLM schema and the CLI flags derive from one list. That is worth doing and is
not blocked by anything; it is simply larger than the change that surfaced it.
Until then the family stays hand-written **together** (the standalone
`run_thread` / `run_coding_agent` / `follow_up_child_thread` LLM tools, the three
`lucidos threads` arms, the hand-written routes), which is at least internally
consistent: no operation is half-generated.

## Alternatives considered

- **Per-tool hand-wiring (status quo).** Rejected — it *is* the drift that caused
  this; nothing keeps the surfaces aligned.
- **Convention + `/harden`-only.** Rejected — not deterministic; drift slips
  through whenever the agent doesn't notice.
- **A generic "run any `lucidos` CLI command" LLM tool.** Rejected — the
  in-process chat agent would spawn a subprocess and round-trip through its own
  HTTP to do what a direct function call does, losing typed args, circuit
  breakers, and per-tool permissions; and it makes the LLM surface a *different
  shape* than UI/SDK rather than in sync.
- **One LLM tool per operation (no grouping).** Rejected — grows the flat tool
  list and worsens model selection; grouping moves the choice to a domain + a
  constrained operation enum.
