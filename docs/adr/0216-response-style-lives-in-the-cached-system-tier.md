# 0216: The response style is a workspace preference, so it rides in the cached system tier

- **Status**: Accepted
- **Date**: 2026-09-18

## Context

Two testers on the packaged build reported the same thing on one evening. The
default chat model goes in circles, and asking it to stop does not stick. One of
them was drifting off the default model to escape it.

Lucidos picks the default chat model and writes the system prompt around it. It
shipped no control over how much came back. The one workaround was a style rule
hand-written into `artifacts/user_profile.md`. No ordinary user finds that.

So a *response style* had to reach the chat system prompt every turn. Where it
sits is the whole question. ADR 0084 moved the clock OUT of the cached system
tier, to stop a turn-boundary rewrite. A value in the wrong tier would undo that.

## Decision

Three decisions. The third is the one most likely to be re-opened.

1. **The style section sits INSIDE the cached system tier**, resolved from two
   workspace-global preferences at turn setup.
2. **There is no per-thread override.** The scope is the workspace.
3. **A trigger cannot pin its own style**, unlike its model and its reasoning
   effort. Follow-up work rather than a refusal.

## Rationale

ADR 0084 bans two things from the system block: a value that varies per turn,
and a value that varies per thread. It states the positive form in the same
breath, and that clause is what this rests on. The block is a function of
workspace state and preferences, and of nothing else.

`response_style` and `response_styles` are workspace-global preference rows. So
the section is a function of exactly what the block may hold. It is
byte-identical across every thread in the workspace, and across every turn of
each one. That is what `two_threads_in_one_workspace_share_the_system_block`
checks. It costs no cache write.

**Standard, the default, renders the empty string.** The placeholder sits at the
end of a line rather than on one of its own. Resolving it to nothing therefore
leaves the prompt byte-identical to a build that never had the setting. A
workspace that never opens Settings pays nothing and reads nothing new.

**The per-thread override is the interesting no.** A per-thread value in the
system block is exactly what ADR 0084 forbids. Both ways around it cost more
than the feature is worth. Moving the style to the turn tail makes every
workspace pay an uncached block per turn, so that a minority can override.
Leaving it in the block and letting threads disagree takes the full system-tier
rewrite ADR 0084 priced at roughly $0.13 a boundary.

There is also nowhere to put it. Model and reasoning effort get per-thread
memory free, by riding on the `MessageReceived` that starts a turn. A style is
not a routing parameter. So it would need its own persisted per-thread state,
and its own in-thread control.

**The trigger pin is a cost decision, not a design one.** A trigger's system
tier already differs per trigger, because the trigger addendum is appended after
every unconditional section. So a pin there would spend no cache that is not
spent already.

What stops it is plumbing. It reaches `TriggerConfig`, `TriggerDefinition`, the
payload readers, both HTTP handlers and the trigger edit form. It also reaches
`TriggerContext` and the `triggers` LLM tool schema, which sits on a documented
per-tool ratchet. The global preference already reaches every trigger fire,
which is what was asked for.

## Consequences

- The always-loaded prompt budget rose by 382 characters, billed at the widest
  SHIPPED style. A style the user writes is workspace content, the same class as
  `user_profile.md`, and is not on that meter.
- **The style library is a preference document, not a table.** No migration, no
  new `SystemEvent`, no CRUD routes. The cost is that `PreferencesChanged`
  persists the whole value. Every save therefore appends the entire library to
  the event log: about 21 KB at the bounds, against one row's worth for a table.
  Those bounds are what hold it there.
- The shipped instructions live in Rust alone. `GET /api/v1/response-styles`
  serves the merged library, so the Settings editor can show them and reset to
  them without holding a copy that drifts.
- **Adding a per-thread or per-trigger style later means moving the section out
  of the cached tier first.** That is a re-read of ADR 0084, not an extra field.
- Coding-agent sessions are untouched. Both keys are kept out of
  `agent_context::SAFE_PREFERENCE_KEYS`, so neither reaches the
  `[USER DEVICE & PREFERENCES]` block either. ADR 0273 later added technical
  literacy as the style's second part, and that part does reach them.

## Alternatives considered

**A `response_styles` table, mirroring the model registry.** The conventional
home for user records. It buys per-row audit events and a natural CRUD surface.
Rejected on size for now: a migration, a store module, CRUD routes, three
`SystemEvent` variants, an announced-surfaces entry, an SSE arm and a frontend
version counter. That roughly doubles the change. It stays the clean migration
if the library grows enable switches, sharing, or the trigger pin.

**A fixed set of named settings with no editing.** The first shape, rejected by
the maintainer during planning. A user who dislikes the shipped wording has no
move. That is the `user_profile.md` workaround again, with extra steps.

**One free-text box instead of a library.** Simpler. It loses the shipped
wording that makes the feature usable for someone who does not want to write
prompt text. The library keeps both: the presets are the starting point, and
editing one is how it becomes theirs.

**Letting the user edit Standard.** Rejected: an editable "add nothing" is a
contradiction. Standard is also what guarantees no workspace changes on upgrade.
The write gate refuses an entry claiming its id. The merge ignores one anyway,
for a row written before that gate existed.

**Letting a style instruction stand alone in the prompt.** Rejected. The engine
appends a floor to every style, outside the editable text. It keeps every
warning, every caveat that changes the answer, and every step the user must
take. "Answer in one line" is a reasonable thing to ask for. It is a dangerous
thing to obey without that rail.

**A numeric verbosity slider.** Rejected before the first commit. Nobody can
calibrate a 1-to-10 scale, and a number says nothing about what it does to an
answer. Each style carries a one-line description instead.
