# 0030: `make lint` gates rustfmt, and the tree was swept once to make that possible

- **Status**: Accepted
- **Date**: 2026-07-30

(This entry uses colons where its neighbours use dashes. `.claude/rules/no-em-dashes.md`
bans the character outright and grants no exemption for house style.)

## Context

Formatting was pure convention. `make fmt` and `make fix` existed, nothing ran
them, and `make lint` (the repo's one lint gate, run per change by `/harden`
Phase 4.5) covered ShellCheck and clippy only.

Measured at `a0c986952`:

| | |
|---|---|
| Tracked `.rs` files | 614 |
| Files `cargo fmt` would reformat | **424 (69%)** |
| Reformat hunks | **1,940** |
| By crate | engine 375, e2e 26, cli 11, gateway 8, app 4 |

The question was raised as an aside and initially waved off as out of scope,
which is why it is worth recording: the tree not being clean is exactly the
argument for a gate, not against one.

Two things made the timing unusually good. `rust-toolchain.toml` already pinned
the channel with `components = ["clippy", "rustfmt"]`, so rustfmt's output was
already reproducible across machines and the "formatter output drifts between
versions" objection did not apply. And the collision surface was empty: three
branches were ahead of `main`, the only live one touched zero `.rs` files, and
the two that touched Rust were seven weeks old and abandoned.

## Decision

**One mechanical `cargo fmt --all` commit across the tree, then
`cargo fmt --all --check` wired permanently into `make lint` as a `lint-fmt`
target**, ordered between `lint-shell` and `lint-rust`.

Three supporting decisions, each of which had a plausible alternative:

- **No `rustfmt.toml`.** Stock defaults.
- **The CLI codegen emitter formats its own output** rather than the generated
  file being excluded or hand-tuned.
- **The `lint-fmt` cargo call carries no `--locked`**, against ADR 0020's
  otherwise-blanket rule.

## Rationale

**The em-dash precedent does not transfer, and that is the crux.**
`.claude/rules/no-em-dashes.md` refuses a retroactive sweep and enforces itself
diff-scoped instead. The reasoning there is that ~29,000 prose substitutions are
a judgment call per hunk, with no way to tell a safe one from a wrong one at a
glance. Neither half holds for rustfmt: it is deterministic and
semantics-preserving by construction, and the entire sweep is verified by one
re-run plus the existing suites. What survives from that precedent is only the
in-flight-branch collision cost, which is a timing problem rather than a
permanent one, and which happened to be zero.

**A diff-scoped gate costs more than it saves.** It buys a smaller one-time
diff. It costs a bespoke script maintained forever (the em-dash rule needed a
shared scan lib, two gate scripts, and a test) plus a tree that stays 69%
unformatted indefinitely. Upstream already ships the gate we want:
`cargo fmt --check` needs no script at all.

**No `rustfmt.toml`, because on a stable channel a config file is a footgun.**
rustfmt accepts a file containing a nightly-only key, prints a warning, and
continues, so a silently-inert setting reads as an active one:

```
$ rustfmt --check --config-path rustfmt.toml t.rs     # ignore = ["x"]
Warning: can't set `ignore = ...`, unstable features are only available in nightly channel.
```

The toolchain pin is what makes stock defaults reproducible, so the config file
would buy nothing and risk that.

**The generated file was the one real design problem.**
`crates/lucidos-cli/src/generated/mod.rs` is tracked and code-generated, and the
gate puts it in a vise: `generated_cli_commands_is_up_to_date` asserts the
on-disk bytes equal `generate_cli_rs()`'s output, while the gate asserts those
bytes are rustfmt-clean. Formatting the file by hand breaks the first; leaving
it breaks the second on the next regeneration. Exclusion is unavailable, because
rustfmt's `ignore` key and a module-level `#![rustfmt::skip]` are both
nightly-only. So the emitter pipes its output through `rustfmt`. Hand-tuning the
templates was the alternative and is strictly worse: the drift was entirely
width-driven, so it would have held only until a manifest entry arrived with a
longer name.

**`--locked` is absent because `cargo fmt` rejects it** (`error: unexpected
argument '--locked' found`). It resolves no dependencies, so there is no
lockfile to drift against and ADR 0020's concern does not arise. This is noted
at the call site so nobody "fixes" it later.

## Consequences

- `make lint` fails on any tracked `.rs` file that is not rustfmt-clean, and
  says to run `make fmt`. Because `/harden` Phase 4.5 already routes `.rs` and
  `Makefile` diffs to `make lint`, every future change picks this up with no
  further wiring. Nothing was added to GitHub Actions, which stays release-only.
- **A toolchain bump can now reformat the tree.** A `rust-toolchain.toml`
  channel bump that moves rustfmt's output reds the gate, so that commit may
  have to carry a `make fmt` sweep. Recorded in the pin's own header and in
  `.claude/rules/build-release.md`.
- **`git blame` on 424 files gains one mechanical layer.** `git log --follow` and
  `git blame --ignore-rev` both take the sweep commit if it gets in the way. The
  sweep was committed alone, with no behavior change and no hand edit, precisely
  so it can be skipped wholesale.
- Generated Rust now has a standing obligation: emit rustfmt-clean output. Any
  future generator that writes a tracked `.rs` inherits the vise described above.
- `make fmt` and the `cargo fmt` inside `make fix` gained `--all` so the
  remediation covers exactly what the gate inspects.

## Alternatives considered

- **A file-scoped diff gate ("format-on-touch"): every `.rs` file your branch
  modifies must be clean.** No tree-wide commit, no collisions, and it mirrors
  the em-dash decay model. Rejected: touching a one-line fix in a 21-hunk file
  drags 21 unrelated reformat hunks into the change, which is a worse review
  experience per change than the one-time sweep is overall, and the tree stays
  half-formatted with a script to maintain forever.
- **A line-scoped diff gate**, intersecting rustfmt's reported hunk ranges with
  the branch's changed lines. Keeps every diff minimal. Rejected: the most
  machinery of the three, and inherently approximate, because a reformat your
  line causes can begin above it.
- **Do nothing.** Rejected: it is the status quo that produced 69% drift, and the
  cost of fixing it only grows.
- **`rustfmt.toml` with `ignore` for the generated path.** Rejected on test: the
  key is nightly-only and warns rather than fails, so the gate would have looked
  configured and been inert.
- **Hand-tuning the codegen templates** so the emitted file happens to be
  clean. Rejected: width-driven drift re-breaks on the next long name.
