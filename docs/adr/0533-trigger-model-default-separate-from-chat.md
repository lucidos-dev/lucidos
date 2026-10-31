# 0533: A trigger default model, trigger_model, is separate from the chat default; chat moves to Opus 5.5 and unpinned triggers run Sonnet 5.5; amends 0515

- **Status**: Accepted
- **Date**: 2026-10-20
- **Amends**: [0515](0515-model-defaults-move-to-current-cheaper-models.md) (the chat row, and its rejected "Opus 5.5 as the chat default")

Record: `docs/plans/2026-10-20-trigger-default-model.md`.

## Context

An intent trigger with no pinned model ran on the account `chat_model`. Chat
and triggers shared one default. ADR 0515 chose Sonnet 5.5 for both, because
Opus costs twice as much and triggers fire unattended.

The maintainer wants a stronger chat default and a cheap trigger default:
"chat default opus med, trigger default sonnet hi".

## Decision

| Purpose | Default | Effort |
|---|---|---|
| Chat (`chat_model`) | `claude-opus-5-5` | `medium` |
| Unpinned intent trigger (`trigger_model`) | `claude-sonnet-5-5` | `high` |

`trigger_model` is a new global preference. A trigger that pins its own model
keeps it.

## Rationale

- **The cost worry was about triggers.** ADR 0515 kept Opus out of the chat
  default mostly because every unattended fire would pay for it. A separate
  trigger default answers that, so chat can lead with the stronger model.
- **Effort needs no new preference.** It stays per model in
  `chat_reasoning_efforts`. The registry already seeds Opus 5.5 at `medium`
  and Sonnet 5.5 at `high`, which is the requested pair.
- **An unreachable default never fails a fire.** When `trigger_model` is
  unset and its default is on no configured backend, the fire follows the chat
  resolver. This keeps the spirit of ADR 0403: a default is a preference, not
  a requirement.

## Consequences

- **Existing workspaces follow the new defaults.** Chat moves to Opus 5.5 and
  triggers with no pin move to Sonnet 5.5. No migration pins either. The
  `CHANGELOG.md` entry names both settings.
- **A user tier stored for a model applies in chat and triggers alike.** A
  user who sets Sonnet's effort once gets it in both places.
- **Settings shows two rows**, Chat model and Trigger model. The trigger form
  shows the trigger default as its Default row.

## Alternatives considered

- **Keep one default and let users pin each trigger.** Rejected. Every new
  trigger would start on the chat model, so the cost default is wrong until the
  user acts.
- **A separate effort preference for triggers.** Rejected. Effort is a
  property of the model, and the registry seeds the requested values.
- **A migration that pins existing workspaces to their current chat model.**
  Rejected. ADR 0515 already let existing workspaces follow new defaults, and a
  pin would hide the change the maintainer asked for.
