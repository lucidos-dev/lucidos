# 0324: Side questions ask a copy of the session, never Claude Code's text-only side_question request

- **Status**: Accepted
- **Date**: 2026-09-29

## Context

The user asked for images on side questions. Claude Code's headless `/btw`,
the `side_question` control request, takes one string: its validator says
"question must be a string". So an image cannot reach it.

The user also asked for one path, not a text path beside an image path. The
plan is `docs/plans/2026-09-29-images-on-side-questions-and-answers.md`.

## Decision

Every Claude Code side question, with or without images, asks a copy of the
thread's session. The copy is the session's own command resumed with
`--no-session-persistence`, under a settings file whose PreToolUse hook refuses
every tool. It takes one stream-json user message: a framing block, the
question, then each image. Its `result` is the answer.

The copy keeps the session's tool list and appended system prompt. The engine
remembers each spawn's appended prompt per thread for this, in memory.

## Rationale

A stream-json user message carries image blocks, so one path serves text and
images. Probes on Claude Code 2.1.280 measured what keeps the prompt cache:

| Resumed copy | Cache read | Cache written |
|---|---|---|
| Same tools, clean tree | 17,925 | 625 |
| Same tools, tree changed since the session began | 18,550 | 0 |
| `--tools ""` | 0 | 8,748 |
| Same tools, a PreToolUse hook denying every tool | 36,491 | 777 |

The tool list sits at the front of the cached prompt, so removing tools
invalidates everything after it. A deny-all hook changes no prompt text, so it
refuses tools and keeps the cache. The refused call came back denied, nothing
ran, and the copy still answered.

## Consequences

- A side question costs a process spawn and a transcript resume, a few seconds
  more than the native request took.
- The copy sees the transcript Claude Code has saved. That holds the running
  turn's finished steps, not the step still streaming.
- The engine keeps the appended prompt in memory only, for up to 128 threads.
  After a restart, or for an evicted thread, every side question until the
  thread's next spawn misses the cache and answers without that prompt.
- The copy is a real model call, so its `result` usage is recorded as a
  `ContextCaptured` with purpose `side_question`.
- The user's own Claude Code settings still load, so their hooks fire on the
  copy's turn too.
- The driver no longer has a side-question channel, pending map or reply
  parser.

## Alternatives considered

- **Keep the native request for text and the copy for images.** Rejected by
  the user: two paths for one feature.
- **The copy with `--tools ""`.** The simplest way to deny tools, and it reads
  nothing from the prompt cache (table above).
- **Always the native request, with images described in text by a vision
  model.** Fast and cheap, but the model never sees the picture, and details
  the user asked about get lost.
- **Deny tools with `--disallowedTools`.** That also removes them from the
  prompt, which breaks the cache the same way `--tools ""` does.
