# 0565: System knowhow stays current through gates that read the code

- **Status**: Accepted
- **Date**: 2026-10-23

## Context

`system-knowhow/` ships to every install, and every user's agent reads it as
fact. The [audit](../audits/2026-10-23-system-knowhow-audit.md) found 2 high,
19 medium and 28 low findings. The trigger was ADR 0560: successor threads
were retired, yet the term still read
as current in knowhow, comments, UI copy and tests.

Today one rule (`.claude/rules/system-knowhow.md`) asks a change to update its
doc, and `/harden` judges it. One script, `check-knowhow-refs.sh`, checks paths,
knowhow ids, and event names in the two audit recipes. It runs only in
`/harden`.

The audit sorted its findings by the gate that missed them. Six classes are
mechanical:

- a retired term written as current;
- a tool that is an alias or does not exist;
- an event row missing;
- an enum value in the wrong case;
- a `path::symbol` that moved;
- a CLI value or flag that drifted.

Two classes need judgment: a wrong default or mechanism, and a stale UI path.

## Decision

Knowhow currency is enforced by **gates that read the code**, in the existing
pipeline. An agent's memory is the last line, not the first.

1. **A retired-terms registry.** `docs/retired-terms.tsv` lists each retired
   term, its replacement and its ADR. `scripts/check-retired-terms.sh` scans the
   whole tracked tree. A hit passes only on a line that names it legacy or
   retired, or in a history path (`CHANGELOG.md`, `docs/adr/`, `docs/plans/`,
   `docs/audits/`, migrations).
2. **One generated agent surface.** `scripts/codegen.sh` writes
   `docs/generated/agent-surface.json` from the code: the clap tree with flags, the offered
   tools with their aliases and retired names, the event type names with
   retired aliases, and the SDK namespaces. This tracked snapshot lets lint
   run without cargo, outside the application's generated build outputs.
   The codegen fingerprint includes CLI sources as well as engine inputs.
   A CLI-side writer reads clap; codegen combines it with the engine rendering.
3. **Forward and reverse name checks.** `check-knowhow-refs.sh` gains arms that
   read the agent surface.
   - Forward: every CLI path, flag, tool name and event name in knowhow exists.
     An alias or retired name appears only on a line marked as one.
   - Forward: a `path::symbol` must resolve, and a `Settings → …` path must
     match the settings search index.
   - Reverse: every live `ThreadEvent` has a row in `thread-events.md`, every
     visible CLI subcommand a section in `lucidos-cli.md`, and every SDK
     namespace a section in `js-sdk.md`.
4. **Generated value lists.** The wire values of payload enums in
   `thread-events.md` sit between generated markers, written by codegen. Nobody
   types a value the code already spells.

   Codegen caches block fragments and splices them into the current document;
   it never caches or replaces the handwritten prose.
   `scripts/check-knowhow-generated.sh` compares committed snapshots and blocks
   with a fresh rendering in temporary output. Any mismatch fails `/harden`.
   `hs_generated_drift` selects suites; it does not certify committed output.
5. **A diff-scoped coupling gate.** The surface table in the rule becomes data,
   `docs/knowhow-surfaces.tsv`. `scripts/check-knowhow-coupling.sh` fails a diff
   that touches a surface without its knowhow, unless a commit trailer
   `Knowhow-unchanged: <surface>: <reason>` says why. Each workspace-audit
   section declares its `sources:`, so a changed source pulls in its audit
   section the same way.

Items 1 and 3 run in `make lint` as a new `lint-knowhow` target, and in
`/harden`. Its unconditional gate list calls `make lint-knowhow` for every diff.
Item 5 is diff-scoped, so it runs in `/harden` only. Items 2 and 4
ride on codegen, with the comparison gate in `/harden` only. Judgment stays
in `/harden`, now pointed at the right files.

## Rationale

- **The code is the authority, so the check reads the code.** A list typed by
  hand drifts the way the docs did. Clap, the capability manifest and the event
  enums already know the truth. One generated file carries it to a fast check.
- **Whole-tree, not diff-only, where the tree can be clean.** The drift in this
  audit came from the other side: code moved and no doc was touched. A diff gate
  on knowhow files would never have fired.
- **The knowhow checks run for every diff.** `make lint` includes them;
  `/harden` also calls `lint-knowhow` unconditionally, including for knowhow-only edits.
  The fast checks read tracked data. Only the freshness comparison may compile.
- **Coupling needs an escape, and the escape must be visible.** Many surface
  edits change no documented behaviour. A trailer with a reason costs one line,
  and a reviewer can judge it. A silent pass would teach nothing.
- **Legacy stays sayable.** Old rows still render, so the code must still name a
  legacy successor thread. The marker word keeps that honest without an
  allow-list of paths.

## Consequences

- A retired concept costs one registry row in the change that retires it. The
  glossary rule says so.
- A stale snapshot fails `/harden`'s freshness gate. After regeneration, a new
  CLI subcommand, event variant or SDK namespace fails lint until its doc exists.
  That is the intended friction.
- A doc can no longer cite a tool by its alias as if it were current.
- `thread-events.md` gains generated blocks. Editing inside the markers is
  pointless, since codegen rewrites them.
- The rule's surface table moves to a TSV. The rule links to it rather than
  copying it, per one definition per value.
- Semantic drift (a default, a mechanism, a UI label outside the search index)
  still needs a reader. The coupling gate makes sure one looks.

## Alternatives considered

- **A rule in CLAUDE.md plus a `/harden` step, alone.** Rejected: that is
  today's arrangement, and the audit is what it produced.
- **Hand-kept lists of tools and commands inside the check script.** Rejected:
  a second copy of a value the code owns.
- **Rust tests in the engine and CLI crates instead of a script.** Rejected as
  the main gate: they run only when their crate's sources change, while drift
  also enters through knowhow-only edits. The generated surface keeps the Rust
  truth and the fast tree-wide scan.
- **Generate whole knowhow files from code.** Rejected: knowhow teaches why and
  when, which code cannot write. Only value lists are generated.
- **Require a doc for every LLM tool.** Rejected: a tool's schema description
  already reaches the model on every call. A second doc per tool would add
  load cost and a second copy.
- **A write-time hook like the em-dash hook.** Deferred: Codex has no hooks, and
  `make lint` covers both backends.
- **A scheduled LLM re-audit of the corpus.** Deferred: worth having for the
  semantic half, but it is a workspace trigger, not a gate.
