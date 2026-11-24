# 0764: The agentic loop buckets its repeat call breaker by what each call does

- **Status**: Accepted
- **Date**: 2026-11-22
- **Related**: [0103](0103-context-trim-passes-and-the-persist-on-demand-verdict.md), [0246](0246-parallel-reads-report-each-call-live.md)

## Context

The agentic loop in `helpers.rs` guards against a model repeating the same tool
call. It also turns raw tool results into model-facing blocks and persisted
events.

Some tools make a bare tool name a poor breaker key. Some results carry large
base64 images that the model and the event store need in different forms.

This record restates the design comments of
`crates/lucidos-agents-api/src/engine/agentic_loop/helpers.rs`, written before
the comment strip removes them. Nobody checked it line by line against the
code.

## Decision

- The consecutive-call breaker key is the tool's target argument: `path`, `url`
  or `query`.
- `run_bash` keys on the first whitespace-delimited token of `command`.
- `run_python` and `run_python_background` key on the first non-blank,
  non-comment, non-import line of `code`, cut to 80 chars. With no such line
  they fall back to the tool name.
- `read_file` adds a suffix when `start_line`, `line_count` or `offset` is
  present. Each (path, window) pair is its own bucket.
- `split_tool_result` turns one raw result into an `llm_text` and an event
  text. `[GENERATED_IMAGE:<b64>]` sends its bytes to the event's `images`
  array. Both texts get the remainder.
- `[IMAGE_CONTENT:<type>]` keeps its sentinel for the model so
  `build_tool_result_blocks` can lift it. The event gets a stub.
- `[APP_CAPTURE:<b64>]` is treated like `[IMAGE_CONTENT:<type>]`, and the event
  keeps the DOM text. Any other result goes to both sides verbatim.
- At most eight images the model explicitly asked to see stay pinned in vision.
  The oldest pin is released beyond that.
- The Flash image-description task is awaited by a free function that takes
  cloned handles. It emits one `ImageDescribed` per hash on the origin message.

## Rationale

- Keying by bare tool name would trip the guard on three unrelated shell calls
  such as `git status`, `git add` and `git commit`. The first-token key keeps
  the intent of stopping an identical call spammed.
- Python retry storms such as `time.sleep(N)` polling share a first actionable
  line, so the generic three-strike guard still fires.
- Paging through one large file is legitimate. Keying page reads by `path`
  alone would trip the `read_file` breaker on page three even though each call
  returns new content.
- The model needs the raw sentinel so its base64 becomes a real
  `ContentBlock::Image`. The event needs a stub because a megabyte of base64 in
  the events table is dead weight the frontend never reads. One 46 MB thread
  froze iOS PWAs.
- Pinned images are exempt from trim pass 0. A turn that describes every photo
  in a folder could pin hundreds of images at about 1600 tokens each and blow
  the context window.
- Eight covers the realistic reason to hold several images at once, which is
  comparing a handful.
- The description task has two callers with different lifetimes. The loop
  awaits it inline after the first LLM call. The chat injection fast-path
  returns to the HTTP caller at once and spawns it as a detached task.
- Hashes come from the persisted `MessageReceived` row, so images whose decode
  failed at emit time are not re-included.

## Consequences

- Unrelated commands run in sequence, while a repeated command prefix still
  counts toward the breaker.
- A pure-import or pure-comment Python call still counts toward the breaker,
  because the key falls back to the tool name.
- The model keeps seeing the images it asked for, and the events table holds
  stubs instead of base64.
- The model is not shown its own synthesised `[GENERATED_IMAGE:<b64>]` image
  back.
- The event text is modelled as an override, `None` meaning it equals
  `llm_text`. The common path stays a single allocation, which matters because
  a `run_bash` result is routinely 150 kB.
- A released pin reverts to the usual rule of stripping an image once it is no
  longer last. The model can call `view_image` again to bring it back.
- The worst case of eight pins is near 13k tokens. That is small against any
  supported context window and is counted by `estimate_message_chars`, so the
  other trim passes compensate.
- An image attached to a message that arrives mid-turn now gets an
  `ImageDescribed` event. Emitting nothing for a missing, empty or bad
  description is the intended no-op.

## Alternatives considered

- Keying the breaker by bare tool name was rejected. It trips on unrelated
  `run_bash` calls and on paged `read_file` reads.
- Collapsing the model text and the event text onto one value was rejected. It
  broke both directions at once: persisting base64 bloats the events table, and
  stripping it blinds the model.
- Leaving the pinned images uncapped was rejected, because a single turn could
  accumulate hundreds of un-strippable images.
- Making the image-description await a `&LucidosEngine` method was rejected.
  The injection fast-path cannot use it from a detached task.
