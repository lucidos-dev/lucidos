# 0388: A fake Claude Code runs the e2e specs that need no real session on GitHub; a daily drift check keeps it honest

- **Status**: Accepted. Amends ADR 0382, which kept every Claude Code spec on the maintainer's Mac.
- **Date**: 2026-10-08

## Context

ADR 0382 kept 12 browser spec files on the maintainer's Mac as the local leg.
They spawn Claude Code, and a runner has no logged-in one. The workflow
holds no secret, so logging one in on GitHub was never an option. The local leg
holds this host's e2e lock and its memory for as long as those files run.

A survey of the 12 found that none edits files, commits, or needs a permission
or question card. Eight need only a session that reports `system/init` and
answers `Say exactly: "X"`. Four need real model behaviour: interrupt then
resume one session, a message arriving mid-tool-call, memory across `--resume`,
and a real side question.

## Decision

**A committed fake CLI answers Claude Code on GitHub's browser shards, and a
spec that needs the real CLI says so with a tag.**

- `crates/lucidos-app/e2e/fake-claude-code/claude.mjs` speaks the stream-json
  lines the engine parses. A prompt it has no rule for gets a fixed reply, so a
  spec that needs a real model fails loudly.
- A test that needs the real CLI carries the Playwright tag `@real-claude-code`.
  GitHub mode keeps its file on the local leg (`e2e_spec_needs_real_claude_code`).
  Today that is four files. The shard `plan` step refuses a tagged file.
- Each browser shard links the fake to `$HOME/.local/bin/claude`, the first path
  the engine probes. It does so only when `GITHUB_ACTIONS=true`, and never over
  an existing file.
- `drift-check.mjs` replays the same turns against the real CLI and the fake and
  diffs what the engine parses. The workspace trigger `claude-code-fake-drift`
  runs it at most once a day, when the installed version changed.

## Rationale

The fake moves two thirds of the local leg onto runners without a secret, so
ADR 0382's zero blast radius holds. The tag replaces a guess with a statement:
the old routing grepped for the composer helper, which every coding-agent spec
calls whether it needs a real model or not.

A fake is only worth its specs while it matches the real CLI. Claude Code
releases often, so the check runs on a version change rather than on a schedule
alone. A daily cap means several releases in one day cost one check. One check
is two short Haiku turns, a small fraction of one local leg.

The fake goes in by binary discovery rather than by the
`coding_agent_claude_path` preference. The engine probes for Claude Code at
boot, before a preference could be written, and the e2e database is recreated
between projects.

## Consequences

- The local leg runs four files, so it holds the e2e lock for less time.
- The eight moved files test the engine against our model of Claude Code. A CLI
  change outside the replayed turns shows up only in the four local files and in
  real use.
- A new spec that spawns Claude Code runs on GitHub against the fake unless it
  is tagged. If it needs a real model, it fails there on the fixed reply.
- A local run, `--local` or the local leg, still uses the real Claude Code for
  every file.
- Codex has no session-level e2e. It is separate work.

## Alternatives considered

- **Fake all 12 files.** That would mean imitating resume, mid-turn queueing and
  side questions. It tests our own model of the hardest Claude Code behaviour,
  where a real regression is most likely. Rejected.
- **Keep all 12 local.** No drift risk, but the local leg keeps the lock and the
  memory for every file. Rejected by the maintainer.
- **Set the preference to the fake.** It must be written after boot and after
  every database reset, and the boot-time model probe would miss it. Rejected
  for binary discovery.
- **Run the drift check in `/harden`.** It needs a logged-in Claude Code and
  spends tokens on every change. Rejected for the trigger.
