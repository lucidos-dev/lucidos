# 0605: Claude Code's progress notes are shown by version, and the Vertex relay is gone

- **Status**: Accepted (the version rule is superseded by
  [0608](0608-claude-code-notes-start-hidden-until-one-arrives.md); the
  Vertex relay removal stands)
- **Date**: 2026-10-31

## Context

ADR 0260 added an engine relay so Vertex calls from Claude Code would carry
`thinking.display: "updates"`. Claude Code 2.1.280 sent it on Anthropic's own API
only. A later change asked `claude auth status` which provider a session used,
and hid the notes on every route but two.

Claude Code's own gate has moved. Reading its bundled code:

- 2.1.300 and 2.1.306 send `updates` on every provider except `gateway`, for
  models after Opus 4.7.
- 2.1.311 drops the `gateway` exclusion.
- Both skip it when `CLAUDE_CODE_DISABLE_EXPERIMENTAL_BETAS` holds any
  non-empty value, `0` and `false` included, or the org has the HIPAA policy.

A repro on 2.1.311, with no relay, printed a `thinking` block with text before
the `Bash` call. Events from the dev workspace agree: `CodingAgentThoughtStreamed`
was zero until the day 2.1.311 installed, then arrived in most Claude Code threads.

## Decision

Notes visibility follows the Claude Code version, not the provider. A session
shows its notes when `claude --version` reports 2.1.311 or later and the spawn
env leaves experimental betas on. An unknown, older or unparsable version hides
them, as before. The Vertex relay, its spawn stamp and the cache probe that rode
on it are deleted.

## Rationale

The binary reports its own version, so the engine asks the authority instead of
guessing a provider. One version check replaces a subprocess per
provider, env and account pin, and it reuses the cached version read the spawn
already makes. The relay now serves nobody that a current Claude Code does not.
A hidden-notes fallback stays for old binaries, so an out-of-date install
degrades to the rule that was always safe.

## Consequences

- `gateway`, Bedrock and Foundry users see notes as messages.
- A Vertex user on 2.1.280 to 2.1.299 loses the relay and falls back to hidden
  notes until Claude Code updates.
- The floor errs high: 2.1.307 to 2.1.310 may also send `updates`. They read as hidden.
- `prompt-cache-first-of-turn-miss` loses its Claude Code probe. A new probe would
  sit at a layer every route passes through.
- `cc-reasoning-dormant` closes as "notes only". The real reasoning never comes back.
- Three cases read as shown while Claude Code sends no notes. The agent's short
  note before a tool call then never reaches the user:
  - an org with the HIPAA policy, which Anthropic's server reports to Claude
    Code and the engine cannot read before spawn;
  - the beta switch set in the `env` block of a Claude Code settings file,
    since the engine reads only the spawn env;
  - a provider that rejects `updates`, after which Claude Code retries without it.

  A spawn-time verdict cannot see these. Deciding from the stream (an empty
  `thinking` block on an always-thinking model) can, and is the follow-up.

## Alternatives considered

- **Keep the provider check.** It is wrong for every route outside its allowlist,
  and it needs a subprocess per spawn key.
- **Keep the relay for old Vertex versions.** It carries a listener, a signed
  token and a proxy for a shrinking population that Claude Code's auto-update
  already covers.
- **Enforce a minimum Claude Code version.** It would block users who run an
  older binary for no gain: the hidden rule works for them.

Supersedes ADR 0260.
