# 0655: One name, user, for operator, person and people; a deliberate migration may correct a wrong name in stored events

- **Status**: Accepted
- **Date**: 2026-11-11

## Context

One concept had three names. ADR 0623 called it an *operator* in the gateway,
the engine, TypeScript and the glossary. ADR 0649 and the UI called it a
*person* or *people*: `thread_people`, *person mention*, *people-only
message*, Settings → People. The user wanted the UI to say *Users*. A
half-renamed concept is worse than either name, so one root had to win in
every layer.

The old names were also stored: in the gateway log `gateway-events.jsonl`
(`OperatorAdded`, `operator_id`), in engine event payloads (`operator_id`,
`mentioned_operator_ids`), in the notification push group and in a
draft's composer selection.

## Decision

- **`user` is the one name root**, in the DB, Rust, TypeScript, the wire, the
  SDK, the CLI, the UI and both glossaries. *Person mention* becomes *user
  mention* and *people-only message* becomes *user-only message*. A user still
  carries no authority (ADR 0623 stands). `MessageRole::User` keeps its LLM
  meaning.
- **The user-only message keeps "only" in every code name**: the event
  `UserOnlyMessagePosted`, the module `user_only_messages`. A bare
  `user_message` already names a model's user-role message.
- **Stored history is migrated, not aliased.** The gateway rewrites its log
  once at start and keeps the old file as `gateway-events.jsonl.pre-user-rename`.
  One engine SQL migration rewrites the event types and payloads, the push
  group and the composer selection.
- **Events stay immutable, with one exception**: a deliberate, reviewed
  migration may correct a name we stored wrongly. It changes names only, never
  what happened.

## Rationale

Keeping an alias for every stored name would leave `operator` readable in the
code forever: every reader of a payload would need both spellings. The user's
words: rewriting history here only admits that we logged a wrong name before.
A rename changes no fact the events record, so the property events exist for
(replay to the same state) holds. The tests check exactly that.

## Consequences

- A downgrade below this version cannot read the rewritten gateway log. The
  `.pre-user-rename` copy restores it.
- Three shims remain, all in `docs/temporary-measures.md`:
  - B still accepts the old relay header pair `x-lucidos-relay-operator`.
    Two machines never update at once.
  - The SDK keeps `threadPeople` as an alias of `threadUsers`, because user
    apps call it.
  - Gateway and engine HTTP adapters preserve user identity and pairing
    selections while either side still runs the previous wire contract.
- ADRs 0623 and 0649 keep their wording. They are history.
- *Operator* in other senses stays: shell and query operators, and the TLS
  "operator certificate" a box's administrator supplies.

## Alternatives considered

- **Alias the stored names forever** (serde aliases, SQL views). Rejected: the
  wrong name would never leave the code, and every new reader pays for it.
- **Rename the UI only.** Rejected: the UI would say *user* while the code said
  *operator* and *person*, the drift that prompted this ADR.
- **Keep *person*.** Rejected: the user chose *Users* for the UI, and the name
  root follows what the user sees.
