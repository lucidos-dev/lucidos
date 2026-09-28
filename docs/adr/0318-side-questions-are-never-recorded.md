# 0318: Side questions are never recorded

- **Status**: Accepted
- **Date**: 2026-09-28

## Context

A tester asked to put a quick question to Claude Code while a coding-agent
thread is working. In the Claude Code terminal that is `/btw`. It answers a
side question from the session's full context, with no tools, beside the
running turn. The answer shows in a dismissible overlay and never joins the
conversation.

Headless Claude Code does not register `/btw` as a command. Its stream-json
loop answers a `side_question` control request instead, with the same query
the terminal uses: tools denied, one turn, no transcript write. Lucidos now
sends that request, to the live process or to a short-lived `--resume` process
started with `--no-session-persistence` when the thread is idle
(`engine/agent_session/side_question.rs`, `runtime/claude_code.rs`).

That left one choice open: whether the engine records the side Q&A anywhere.

## Decision

The side question and its answer are never recorded. No thread event, no
database row, no file. `POST /api/v1/coding-agents/side-question` returns the
answer to the asker and forgets it. The card lives in the client's memory until
it is dismissed or the page reloads.

## Rationale

1. **Ephemerality is the feature's contract.** Native `/btw` is an overlay that
   never enters history. Recording it would make it a different feature.
2. **Recorded thread events feed context builders.** The next session's system
   prompt is built from the thread's messages, and recaps are rebuilt from its
   events. A recorded side Q&A would need an exclusion filter in every one of
   those readers, today's and every future one. Recording nothing makes "never
   enters the main context" true by construction.
3. **Dismissal then needs no state of its own.** There is nothing to mark
   dismissed and nothing to sync across devices.

A structural test pins it: `side_question.rs` holds no event bus and emits
nothing (`the_side_question_module_never_emits_an_event`).

## Consequences

- A reload, or another device, shows no card. A side question is asked again
  if its answer is needed again.
- Each side question stands alone. Claude Code's request accepts a `history`
  field for earlier side Q&A, and Lucidos sends none, since it keeps none.
- **Token Cost under-counts side questions, as a documented non-goal.**
  Claude Code's `control_response` carries no usage block, so the engine cannot
  see what a side question cost, and nothing reaches the Token Cost app. This
  is a gap against ADR 0242, which asks every model call to record its cost.
  It stays until Claude Code reports usage for side questions. The cold path's
  likely prompt-cache miss goes uncounted too.
- Codex threads get a plain refusal: Codex's protocols have no side-question
  call, and nothing is ever sent to the Codex session.
- The normal chat route refuses `/btw` in a coding-agent thread, so a caller
  that bypasses the composer gets an error instead of a main-session turn.

## Alternatives considered

- **Record it as a thread event, filtered out of context.** Survives reload and
  shows on every device. Lost on rationale 2. Every context builder, recap and
  export would need the filter. One missed reader leaks the side Q&A into the
  main session, the exact failure the feature exists to avoid.
- **Keep it in client storage (localStorage) to survive a reload.** Still
  per-device, adds dismissal state to sync or leak, and buys little for an
  answer meant to be glanced at.
- **Ask through a forked, resumed prompt turn.** Costs a full turn, can run
  tools, and diverges from what the terminal's `/btw` answers. The native
  control request is the exact primitive, so it won.
