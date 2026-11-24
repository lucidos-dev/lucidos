# 0695: A Codex side-question fork disables every non-managed hook by key and fails closed if one stays enabled

- **Status**: Accepted
- **Date**: 2026-11-15

## Context

A Codex side question runs in an ephemeral `thread/fork` (ADR 0684). The fork
turns `features.hooks` on, so it can run Lucidos's synchronous `PreToolUse`
deny guard, and it checks that guard through `hooks/list` before any model turn.

Codex 0.169 has no switch narrower than `features.hooks`. `codex features list`
reports `plugin_hooks` as `removed`. The app-server schema has no per-event or
per-source switch. So the fork also ran every hook the user configured: user
and project hooks on `SessionStart`, `UserPromptSubmit`, the model-request
events, `Stop` and `SessionEnd`. A "Codex finished" notification hook fired on
every side question, and a prompt hook could inject context into the answer.

Codex does expose a narrower handle. `hooks/list` returns each hook with a
stable `key`, its `source`, `enabled`, and `isManaged`. The session-flags config
accepts `hooks.state."<key>".enabled = false`, the same table the fork already
uses to trust its guard. A probe against Codex 0.169 with a throwaway
`CODEX_HOME` confirmed it: trusted user `SessionStart` and `Stop` hooks and a
trusted project `UserPromptSubmit` hook all listed `enabled: false` in the new
thread.

## Decision

A side-question fork runs only the Lucidos tool guard and hooks Codex marks
`isManaged`. Before the fork, the engine lists the hooks for the fork's `cwd`
and disables every other one by key in the fork's config. After the fork, the
existing `hooks/list` check also refuses the side question if any non-managed
hook other than the guard is still enabled.

## Rationale

- **A side question is a read, not a session.** It never touches the user's
  rollout and never uses a tool. Hooks written for real sessions (notify on
  `Stop`, log on `SessionStart`, inject on `UserPromptSubmit`) misfire on it.
- **Managed hooks are policy.** An administrator's MDM, requirements or cloud
  hooks stay on. Lucidos does not override an admin's controls, and Codex's own
  `isManaged` flag decides membership, not a source list Lucidos keeps.
- **Codex's API is the authority.** The keys come from `hooks/list`, so the
  mechanism holds for every hook source and path layout, on every platform
  Codex runs on. It mirrors how the fork already disables each MCP server by
  name from `config/read`.
- **Fail closed, like the guard.** A future Codex that ignores the override, or
  a hook source that appears only after the fork, would silently bring the
  user's hooks back. Refusing with a clear error keeps the contract checkable,
  the same way a missing guard already fails.

## Consequences

- A user's own logging or audit hook no longer sees side questions. Context
  that a prompt hook injected earlier in the main session stays, because the
  fork carries the conversation.
- The main Codex session is untouched: it runs every hook as before.
- Side questions fail with an "update Codex" error, rather than run the user's
  hooks, on a Codex release that breaks the mechanism. That covers dropping
  `key`, `enabled` or `isManaged` from `hooks/list`, or ignoring session-flags
  state.
- The dead `plugin_hooks` entry leaves the fork config, since Codex 0.169 no
  longer reads it.

## Alternatives considered

- **Accept the user's hooks as intended.** Simplest, but every side question
  fires session hooks the user wrote for real work, including notifications.
- **Wait for a Codex-side switch.** No such flag exists in 0.169 and no date is
  known. The per-key override already does the job through a supported API. If
  Codex ships a source-level switch, adopting it can replace the per-key list.
- **Second pass before failing.** Re-forking with the stragglers disabled adds
  a round trip and code for a case that only a Codex behaviour change creates.
  An "update Codex" error is the honest answer there.
- **Warn and proceed.** Restores today's behaviour silently in exactly the case
  the check exists for.
