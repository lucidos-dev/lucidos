# 0651: Owner-authorized submission of an exact saved draft

- **Status**: Accepted
- **Date**: 2026-11-10

## Context

Agents can read saved drafts but cannot submit one on the owner's instruction.
Copying draft text into an ordinary agent message loses attachments and compose
settings, and changes who authored it. Calling the current chat handler and
then clearing compose state leaves a crash window between acceptance and draft
consumption.

## Decision

Expose one saved-draft submission operation through the threads tool, CLI and
HTTP API. It takes a thread, expected draft revision and epoch, and an
idempotency key; content and settings come exclusively from the saved draft.

Require authenticated agent provenance and owner authority even inside the
caller's subtree. Reuse standing owner instructions and a narrow, single-use
`submit-draft` owner approval; Home receives no special privilege. Persist
owner authorship separately from the submitting agent and its authority
evidence, without creating a Device origin or opening a human-authorized turn.

Validate the whole snapshot and every referenced image before acceptance.
Through EventBus, atomically persist the immutable payload, receipt and
recoverable queue membership while consuming exactly the checked compose
revision. Serialize with ordinary composer sends using their shared admission
and append ordering. Broadcast and wake execution only after commit.

## Rationale

The operation submits content the owner already saved, rather than allowing an
agent to manufacture a user message. Revision and epoch checks protect both
edits within a compose epoch and writes delayed across a send. A durable receipt
lets a retry return its original acceptance even after later edits or a restart.
Database uniqueness prevents two keys from accepting the same draft revision.

These contracts apply to composing and active threads, chat and supported coding
backends, and image-only drafts on every supported install. Missing blobs or
unsupported explicit selections refuse without consuming the draft. Unset
selections follow the composer's defaults.

## Consequences

Saved content uses a `saved_draft` message origin carrying the acceptance event
and submitting thread. Its semantic mode is Human because the owner authored
the content; its origin is never Device. Standing-instruction checks therefore
do not treat execution of the accepted draft as a fresh owner-opened turn.

Success reports durable acceptance, not execution completion. Execution reuses
the existing queue and message identity, without consuming compose state again
or emitting a second user message. Same-key retries are idempotent; reuse for a
different target or revision refuses. Ordinary question answers grant no
submission authority. Failed device-held sends remain outside this operation.

Ordinary composer chat submissions share durable acceptance and recording order
with saved submissions. Their receipts retain the original wire intent separately
from the resolved execution payload. Ordinary work retains user priority without
an additional background reservation; people-only routing records without taking
agent capacity. Acceptance emits no synthetic user message: question answers and
coding redirects keep their execution-time recording rules.

Card answers retain urgent recording: they never wait behind the running turn
that asked the question or behind agent capacity. Composer-bearing card answers
check their captured epoch and consume compose in the answer's append transaction.
The persisted answer carries the resulting compose report for projection rebuilds;
option-only answers do not consume or fence compose state. Duplicate answer writes
roll back consumption with the answer. Clients preserve the captured fence across
quiet retries and only use a successful compose receipt to advance their send chain.

An accepted input retains a recording barrier until its own durable outcome or
an explicit queue completion/drop. Dropping a failed executor cannot release
later inputs ahead of unrecorded content. Recovery rebuilds the shared order and
verifies the queue projection against the immutable accepted payload.

Clients capture compose epochs when the owner forms the send intent. Only a
confirmed immediate ordinary predecessor can authorize a causal rebase. Once a
saved submission exists on a thread, an unfenced legacy send refuses. Unknown
retry outcomes preserve the complete original wire identity; a confirmed refusal
lets an explicit Retry form a new intent without discarding device-held content.

## The revision is the draft revision

The revision a caller names is `thread_summaries.draft_revision`. A trigger
raises it only when a draft column changes. `summary_version` was the first
token, but activity events raise it, so a running thread always read as stale.

## Alternatives considered

- Send copied text as an agent message: loses the owner's exact images,
  selection and authorship.
- Accept arbitrary replacement content with a user-origin flag: permits
  impersonation and does not bind the send to saved owner content.
- Clear compose state before or after an independent chat call: can lose the
  draft or duplicate a send across a failure or restart.
- Give Home blanket permission: widens authority based on the caller's role
  rather than the owner's instruction.
