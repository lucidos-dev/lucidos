# 0239: A session's authority stops at the shipped product's users

- **Status**: Accepted
- **Date**: 2026-09-21

## Context

On 2026-09-20 a session hardening the ADR 0227 app frame measured a real
regression. A nested document inside the isolated frame can no longer
authenticate against the gateway. The frame's site-for-cookies is null, so no
device cookie is sent, and `auth_api::enforce` refuses. `wants_html` then
answers that refusal with `serve_pairing_shell`. So a 62 KB interactive shell
is served as the answer to a nested subresource. The user's own site preview
then paints the Lucidos pairing splash.

The session saw it, understood it, and recorded it in its plan file as a
NON-GOAL. The reasoning was sound: exempting these frames would hand an
unauthenticated reach over workspace files. Then it finished. Nothing carried
the finding to the human. He found it in production a day later, and would
never have approved it.

A second case landed the same day. The clipboard regression from the same
isolation change was written down as a release gate. It shipped in v0.39.0
anyway.

The failure is structural, not a lapse of care. A session closed a question
about the shipped product's security by writing a word in a plan file. A note
is not a gate. Nothing in the pipeline reads a plan file, an ADR, or a commit
message and stops a release.

## Decision

A session may not settle, by itself, a question about the shipped product's
security, data exposure, or user-visible correctness.

When a session finds something it will not fix that reaches users of the
shipped product, it must act. It raises the finding with the question tool
before it finishes, and names it in its final report. Recording it as a
non-goal, a known limitation, a follow-up, a deferred item, or a release gate
does not discharge the obligation. The place recorded does not matter: a plan
file, an ADR, and a commit message all fail the same way.

The bar is deliberate, so this does not become a prompt for every observation.

- It reaches users judged by reachable population plus harm. It is not judged
  by whether it fires on this machine or in this workspace. A shipped feature
  is in use by someone.
- The session is choosing not to fix it, or cannot fix it inside its scope.
- Silent failure, security, data exposure, and data loss raise it. Cosmetic or
  self-evident issues do not.
- A finding the session fixes in the same change needs no question. A finding
  outside scope that is loud and harmless can stay a note.

The rule lives in the engine session prompt, in
`RAISE_USER_REACHING_FINDINGS_RULE`. It is session truth by
`docs/agent-config.md` § Which surface owns a rule, so it reaches every session
the engine spawns.

## Rationale

A coding-agent session has wide authority inside its change. Its authority
stops at the product's users. A choice that changes what a real person meets,
in security, data exposure, or correctness, is the human's to make. The session
can measure it, reason about it, and recommend. It may not decide it alone.

The question tool is the one surface that reaches the human in time. It parks
the thread in the waiting-for-answer state, which lights the needs-attention
badge and can notify the user. A plan file, an ADR, and a commit message reach
a different reader. They reach the next session and the later reviewer, after
the release. So they inform, they do not gate.

The bar is drawn on reach, not on locality. A session often notes that a bug
does not fire on its own machine or workspace. That is the wrong test. A
shipped feature is in use by someone, so a reachable harm has already reached a
user. The bar also excludes the loud and the harmless, which the user meets on
their own.

## Consequences

- A user-reaching security or correctness question surfaces as a decision,
  before the change lands.
- A buried non-goal about such a question is a governance violation. The
  `code-review` skill flags it, so `/harden` catches it in review.
- The rule costs about 1052 bytes on every request of the six chat-style
  prompt flavors. The two conflict-resolution ceilings do not move.
- The merge-conflict session does not carry the rule. It runs unattended and
  has no question-tool section for the rule to point at.
- The session still records the finding in its notes. The question and the
  report are added to good notes, not a replacement for them.

## Alternatives considered

**Leave it to review.** Trust `/harden` and the reviewer to catch a buried
non-goal. Rejected because review runs on the diff, after the session decided.
The session already held the finding and the context. Surfacing it then is
cheaper and more honest than reconstructing it later.

**State the rule only in `CLAUDE.md`.** Rejected because four of the seven
prompt flavors have no Lucidos checkout, so they never read it. The rule is
also session truth, not repository truth: it is true because the engine gave
the session the question tool.

**Put the review check in `docs/code-review-priors.md`.** Rejected because that
file is a dismiss-list of patterns reviewers should NOT flag. An entry there
would wave a buried finding through. The flag belongs in the `code-review`
skill's angles, which `/harden` Phase 1 runs on the diff.

**Widen the rule to every prompt.** Rejected because the merge-conflict session
runs unattended and carries no ASKING USERS section. The rule points at that
section, so it would dangle there.
