# Agent configuration

How this repo's Claude Code and Codex configuration is put together, what each
layer costs, and the gate that keeps it honest.

**This page is for maintainers, not for a session.** It is deliberately not a
rule and not a skill, so it never enters an agent's context. Content that
explains the configuration belongs here; content that instructs an agent
belongs in `CLAUDE.md`, in `.claude/rules/`, or in a skill. Putting the
explanation in front of the agent is how `CLAUDE.md` grew 98% in the seven
weeks to 2026-08-06.

## The four layers

Every coding-agent session assembles its instructions from four places. They
differ in when they load, what authority they carry, and what they cost.

| Layer | Where | Loads | Authority |
|---|---|---|---|
| Engine system prompt | `crates/lucidos-engine/src/engine/agent_session/prompts.rs` | every session | system prompt |
| Always-loaded instructions | `CLAUDE.md` + unscoped `.claude/rules/*.md` | every session | user message, after the system prompt |
| Path-scoped rules | `.claude/rules/*.md` with `paths:` | when Claude reads a matching file | user message |
| Skills | `.claude/skills/*/SKILL.md`, `.claude/commands/*.md` | description every session, body on invocation | user message |

Three consequences worth holding on to.

**The engine prompt outranks `CLAUDE.md`.** Claude Code delivers `CLAUDE.md` as
a user message *after* the system prompt, while the engine appends its rules to
the system prompt itself. Where the two say the same thing, the engine copy is
the authoritative one. They should almost never say the same thing: see
§ Which surface owns a rule.

**`CLAUDE.md` reaches both backends; `.claude/rules/` does not.** Codex reads
`CLAUDE.md`. Both Codex drivers point codex's project-doc fallback list at it
(`CODEX_PROJECT_DOC_FALLBACKS` in `crates/lucidos-engine/src/runtime/codex.rs`,
with `CODEX_PROJECT_DOC_MAX_BYTES` raising codex's 32 KiB cap to 65,536 so the
tail rules are not silently truncated). That is how ADR 0004 got CC parity for
the working agreement without shipping an `AGENTS.md`. What a Codex session does
*not* get is the unscoped `.claude/rules/*.md`, so the em-dash ban and the
private-data rule bind it only through `/harden` and the release guard, after
the fact.

**Only the engine prompt and the always-loaded set are unconditional.** Those
two are the entire budget an agent pays before it has read a line of code.
Everything else is earned: a path-scoped rule arrives with a matching Read, a
skill body arrives with an invocation.

## Where a new instruction goes

Ask when it has to be true.

- **Before any file is touched**, or it governs prose and chat replies rather
  than code: `CLAUDE.md` or an unscoped rule. This is the expensive slot.
  Safety rules and cross-cutting prose rules live here because they genuinely
  cannot be gated on a path. If the rule is about the *session* rather than the
  repository, it belongs in the engine prompt instead: see the next section.
- **Only when working on a particular kind of file**: a path-scoped rule.
- **A procedure, a checklist, or reference material**: a skill. The body costs
  nothing until it is invoked.
- **It must hold every time, regardless of what the agent decides**: a hook.
  A prohibition written in prose is a request; a `PreToolUse` hook that refuses
  the call is a guarantee. The em-dash rule and the kill guard are both hooks
  for exactly this reason, and the prose that accompanies them should be short
  because the prose is not what enforces them.
- **It explains the configuration rather than instructing an agent**: this
  page.

## Which surface owns a rule

Two of the four layers are unconditional, and both reach both backends, so a
rule can physically be written on either. Writing it on both is what actually
happened: six rules were stated twice by 2026-08-06, each one paid twice on
every request and each free to drift out of agreement with its twin. The test
that decides the owner:

> **Is this true because the engine spawned this session, or is it true of the
> repository?**

**Session truth goes in the engine system prompt**, and `CLAUDE.md` says nothing
about it. You are in a worktree on this branch; ending your turn kills your
process group; your commits become a pending change the user Applies; your
reasoning is not rendered; the question tool parks the thread; the plan and
harden markers gate Apply. None of that is knowable from the repository, and the
engine prompt is the only surface that reaches a session with no checkout.

**Repo truth goes in `CLAUDE.md`**, and the engine prompt says nothing about it.
Conventions, code style, test selection, architecture, naming, and the local
hazards of specific repo scripts. It reaches both backends, so despite the name
it is already the backend-agnostic surface.

Two asymmetries make this test decide cases rather than merely describe them.

- **Four of the seven prompt flavors have no Lucidos checkout.**
  `external_repo`, `app_worktree`, and both recovery variants. An app worktree
  is a sparse checkout of the *workspace* git narrowed to one app folder, so it
  has no `CLAUDE.md` and no `scripts/`. A rule those sessions must obey can live
  only in the engine prompt.
- **A hand-run `claude` in this repo gets no engine prompt.** Check where the
  enforcing hook is registered before deciding. A hook in the repo's own
  `.claude/settings.json` (`pre-kill.sh`, `pre-push.sh`, `no-em-dashes.sh`)
  fires for that session, so the rule explaining the refusal must be reachable
  from `CLAUDE.md`. A hook registered only in the engine-generated
  `.lucidos/cc-settings.json` (`cc-plan-gate`) never fires there, which is what
  makes the plan-marker rule pure session truth.

### The one sanctioned mirror

Where both asymmetries apply at once, no single surface reaches everyone the
rule binds, and it has to be stated twice. Exactly one rule is in that position:
the process-safety prohibition (never broad-kill `lucidos-engine`, ADR 0025). It
binds no-checkout sessions, which only the engine prompt reaches, and it is
enforced for a hand-run `claude` by `pre-kill.sh`, which only `CLAUDE.md`
explains.

So the engine prompt carries the full text and `CLAUDE.md` carries a one-line
prohibition. `./scripts/check-prompt-mirror.sh` fails if either half goes
missing, and `/harden` Phase 4.5 runs it for every diff, including the
`CLAUDE.md`-only edit that no `cargo test` would catch.

**A second mirror needs the same proof**: name the populations the rule binds,
show that no single surface covers them, and extend the guard in the same
change. Duplicated prose with no entry in that guard is drift, not a mirror.

### Worked examples

The routing decided on 2026-08-06, as calibration for the next case.

| Rule | Owner | Why |
|---|---|---|
| Worktree isolation, Apply and restart, turn lifecycle, the question tool, the plan marker, `/harden` | engine | Session truth. `CLAUDE.md` deleted its copy of each. |
| Never start a stack from a worktree (ADR 0021) | engine | Session truth, and a hand-run `claude` is not in a worktree, so it is not bound. |
| Never hand-roll HTTP to the engine API (ADR 0050) | engine | Session truth. The incident narrative lives in the ADR, not in front of a session. |
| Commit cadence, no pull requests | engine | Session truth, and never duplicated. `CLAUDE.md`'s conventional-commits bullet is commit *message format*, a different rule, and its "not PR-based" mention is rationale inside the GitHub-Actions bullet, load-bearing for a different claim. |
| Never kill broadly (ADR 0025) | **both** | The one mirror, guarded by `check-prompt-mirror.sh`. |
| A `scripts/lib/*_test.sh` run can kill a live engine | `CLAUDE.md` | Repo truth about specific repo scripts, which no-checkout sessions do not have. |
| Test selection, code style, naming, `Edit` needs a fresh `Read` | `CLAUDE.md` | Repo truth. |

## Rule scoping

A rule file in `.claude/rules/` is loaded one of two ways, decided by its
frontmatter.

- **Path-conditional**: the file declares a `paths:` list, and Claude Code
  loads it when Claude reads a file matching one of the patterns. This is the
  default for a scoped rule and keeps it out of every unrelated session.
- **Always loaded**: no frontmatter at all. Reserved for rules that cannot be
  gated on a path.

**The key is `paths:`, not `globs:`.** A `globs:` key is a Cursor convention
that Claude Code silently ignores, which puts the file in the always-loaded
bucket. The entire rule set was resident in every session until 2026-07-25
because of this, and nothing in a session revealed it. Patterns are **glob**
patterns, not gitignore patterns: `src/**/*.ts` matches at any depth under
`src/`, while a bare `Makefile` matches only the repository root. Brace
expansion works (`src/**/*.{ts,tsx}`). A `paths:` of exactly `**` scopes
nothing and is treated as always-loaded.

Two limits to design around:

- **A path-scoped rule arrives only once Claude reads a matching file.** So it
  is not in context while the change is being planned. Anything that must be
  true *before* the first edit belongs in `CLAUDE.md` or an unscoped rule.
- **Rules load on read, not on write.** Creating a new file from scratch does
  not pull in the rule that covers it (claude-code#23478, closed without a
  fix). A "when creating a file, always X" rule written as a path-scoped rule
  will not fire.

## Sizing a path-scoped rule

Scoping is not free once a rule gets large: the whole file arrives on the first
matching read, so the cost of a rule is its size times how often its scope
fires, and a rule reachable from files it has nothing to say about is paying
that cost for nothing. Three splits on 2026-08-06 and two deliberate
non-splits, as worked examples of where the line is:

| Rule | Before | After | Why |
|---|---|---|---|
| `build-release.md` | 112,310 | 84,157 | The three front-door CI jobs became `front-door.md`. They were reachable from `install.sh`, `Dockerfile` and every build script, none of which can change them. |
| `frontend.md` | 59,374 | 40,634 | The CSS and component-class half became `frontend-css.md`, scoped to `.css` and `.tsx`. A stylesheet cannot change the `Loadable<T>` contract. |
| `Makefile` scope | 172,149 | 59,839 | It was listed in both `dev-runtime.md` and `build-release.md`. `dev-runtime.md` owns it alone now. |
| `db.md` | 62,481 | unchanged | Its scope is migrations only (there are no `.sql` files outside `migrations/`), and a migration author wants the schema. Splitting the event enumeration onto `event_bus.rs` would add 30 KB to every engine event edit to save it on the rarer migration. |
| `dev-runtime.md` | 59,839 | unchanged | Its two largest sections are both dev topology, which is exactly what it is scoped to. No path seam exists that separates a subset of its scripts from a subset of its content. |

The test that decides it: **name a file in the rule's scope that cannot change
anything the rule says.** If one exists, the rule is over-scoped and that
content wants its own narrower rule. If none does, the rule is the right shape
however large it is, and shrinking it means deleting or compressing content
rather than moving it.

Prefer a narrower **rule** to a **skill** for this. A skill body loads only when
the model chooses it from a description, while a path-scoped rule arrives
automatically on the read, which is what conventions want. A new rule file also
breaks no inbound reference: the roughly forty references to `.claude/rules/*`
from `crates/`, `docs/` and `system-knowhow/` cite files by section name, so
moving a section out is only safe after checking which ones name it.

`dev-runtime.md` and `build-release.md` list their scripts **by name** rather
than taking a `scripts/**` catch-all, which is the only way an edit to a build
script can skip the dev-runtime rule and vice versa. The cost is a maintenance
obligation: a new script under `scripts/` matches neither rule until its path
is added to the right one, and a rule that silently fails to load looks exactly
like a rule that does not exist. Add the path in the same change that adds the
script. That obligation is restated in `CLAUDE.md` because it binds before any
file is touched.

## The context budget

`scripts/check-context-budget.sh` measures the always-loaded set and fails on
two arms. `/harden` Phase 4.5 runs it for every diff, docs-only included, since
a docs-only diff is exactly the change that grows it.

- **Size.** The set must stay at or under `CONTEXT_BUDGET_CEILING` in
  `scripts/lib/context_budget.sh`. It is a ratchet: lowering it needs no
  ceremony, raising it needs a reason in the commit message saying what became
  worth paying for on every request.
- **Membership.** The resident set must be exactly
  `CONTEXT_BUDGET_EXPECTED_ALWAYS`. This arm is not about size. It is the
  regression detector for a rule that was meant to be scoped and silently is
  not, and it fires on all four ways that happens: a `globs:` key, a `path:`
  typo, a `paths:` of exactly `**`, and frontmatter that never closes.

`./scripts/check-context-budget.sh --report` prints the set without failing.
Fixing a size failure means moving content, not raising the number.

`.claude/hooks/log-instructions-loaded.sh` records the other half: an
`InstructionsLoaded` hook appending every real load to
`.lucidos/instructions-loaded.jsonl`. The gate reads the tree and says what
*should* load; the log reads a live session and says what *did*. Worth having
because every coding-agent session runs in a git worktree, and there are
upstream reports of `paths:` filtering being skipped through worktree path
resolution (claude-code#23569). Measured on Claude Code 2.1.220, scoping works
correctly here: a session starts with only the unscoped files resident, and
reading `crates/lucidos-engine/src/net_config.rs` pulls in exactly `rust.md`
and `system-knowhow.md`.

## Hooks

Two settings files register hooks, and both apply.

- **`.claude/settings.json`** (tracked, this repo): `PreToolUse` on `Bash`
  (`pre-push.sh`, `pre-kill.sh`, `no-em-dashes.sh`), on `Edit` and `Write`
  (`no-em-dashes.sh`), and `InstructionsLoaded`
  (`log-instructions-loaded.sh`).
- **`.lucidos/cc-settings.json`** (generated per workspace by
  `crates/lucidos-engine/src/engine/cc_settings.rs`, passed with `--settings`):
  `PreToolUse` on `AskUserQuestion`, `Bash`, `Read`, `Edit` and `Write`, a
  `Stop` hook, the default model, and `permissions.additionalDirectories`.
  The hooks call `lucidos` subcommands.

`permissions.deny` in `.claude/settings.json` blocks reads of build output and
one large generated fixture. Content searches already respect `.gitignore`, so
these only matter for a direct `Read` of a path a search surfaced: the
78 KB `cross-validation-fixture.json` is roughly 20k tokens if opened. Deny
rules are not overridden by `--allowedTools`, so unlike `permissions.allow`
below they do apply to engine-spawned sessions.

`permissions.allow` in `.claude/settings.json` applies **only to a manual
`claude` launch in this repo**. The engine passes `--allowedTools` when it
spawns a session, which overrides settings.json permission rules entirely; the
compiled default is empty and the effective list comes from
`<workspace>/.lucidos/cc-allowed-tools`, which is **per workspace** (ADR 0095):
a grant made in one workspace is asked again in the next. See `GrantFile` in
`crates/lucidos-engine/src/core/grants/mod.rs`.

**An allowlist entry does not outrank CC's own path check.** Its Bash evaluator
runs exact rules, then prefix rules, then a path-validation layer, and only then
the allow rules. The path layer returns immediately when it asks, unless it
marked the ask rule-overridable. A `cd` outside the session's allowed working
directories never is, so `Bash` in `cc-allowed-tools` cannot suppress that card.

The engine grants two directories through `permissions.additionalDirectories`
for exactly this reason. `<workspace>/data` holds the artifacts, knowhow, apps
and triggers an agent reaches from a worktree that is their sibling. `/tmp` is
the OS temp dir, where an `Edit` cards with reason `workingDir`. That card hides
its "Always allow" button, because CC honours no bare `Edit` allow rule in any
mode. Both directories live in `engine/cc_settings.rs`.

Four shapes stay unsuppressable at any scope, because no rule reaches them:
`cd` with an output redirection, `cd` with a write command, `cd` before `git`,
and two `cd` in one command. Avoid them by not prefixing `cd`, rather than by
adding rules. Background and the rejected wider scope:
[`docs/plans/2026-08-25-cc-permission-cards-from-cd-outside-the-worktree.md`](plans/2026-08-25-cc-permission-cards-from-cd-outside-the-worktree.md).

**The mode itself is a preference.** `coding_agent_claude_permission_mode` picks
between `accept-edits` (the default, and what every session ran before the key
existed) and `auto`, where CC's own safety classifier approves routine actions.
Auto reaches the four shapes above. It also drops classifier-bypassing allow
rules, bare `Bash` included, and denies rather than cards when the classifier is
unreachable.

The engine sets `CLAUDE_CODE_ENABLE_AUTO_MODE=1` alongside it. Without that, CC's
provider gate downgrades the session to `default`, which cards more than
`acceptEdits` does. The flag rides every spawn, and CC prefers a CLI value to any
settings file. So this preference is the only way to choose the mode. Full
reasoning: [`docs/plans/2026-08-25-claude-code-permission-mode-preference.md`](plans/2026-08-25-claude-code-permission-mode-preference.md).
