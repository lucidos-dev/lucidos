# 0379: A model registry vision flag gates image description: its picker, its default and the call

- **Status**: Accepted
- **Date**: 2026-10-06

## Context

ADR 0375 made every background picker offer every registry model. So
`model_image_description` could point at a model that cannot read images. Every
description then failed, and the only trace was a log line. The registry had no
way to say which models read images. The evidence and phases are in
`docs/plans/2026-10-06-vision-flag-for-image-description.md`.

## Decision

Each `models` row carries a `vision` flag, the *vision flag*. Image description
offers, defaults to and calls only a model whose row has it. A stored pick
without it is refused before the call and reported in Settings, never replaced.

## Rationale

- **Per model, not per route.** Reading images belongs to the model. A route is
  only a backend serving that same model.
- **A plain boolean.** `false` means "not known to read images". A third
  "unknown, try anyway" state would keep the silent failure.
- **The seed errs low.** Over-declaring sends images to a model that rejects
  them, which is the bug. Under-declaring only hides a model from one picker.
  So only the Claude, Gemini and GPT-5/6 builtins are marked, Codex Spark
  excepted. A test makes every later builtin seed decide.
- **No row means no vision.** A Claude Code id, a deleted model or an env-only
  id cannot be checked, and guessing from the id is how the bug would return.
- **Editable on every row, builtins included.** A seed can be wrong or
  unverified, like routes. The user flips it in Settings, the API,
  `manage_models` or the CLI.
- **Refuse before the call.** A model that cannot read images fails on every
  image, so the call would only spend money. The refusal names the model and
  the fix. ADR 0375's rule holds: a stored pick is honoured or refused, never
  substituted.

## Consequences

- `GET /api/v1/models/background` reports `needs_vision` and `vision` per row,
  and the Image description row shows a caveat when they disagree.
- The auxiliary default skips candidates without the flag for image
  description. It falls back to the chat model only to refuse it when that
  model cannot read images.
- Unverified builtins (GLM 5.2, the Grok rows, the OpenCode free rows) stay out
  of the Image description picker until someone flips them.
- An existing workspace whose stored pick lacks the flag stops getting
  descriptions it was already failing to get, and now sees why in Settings.

## Alternatives considered

- **A per-route flag.** Lost: no backend serves the same model with and without
  vision, so it would only add a field to keep in step.
- **Infer vision from the model id.** Lost: ids drift, and a wrong guess is the
  failure this fixes.
- **Let the call fail and log it.** That was the state before. It spends money
  on every image and tells nobody.
- **Fall back to a vision model when the pick cannot read images.** Lost for
  the reason ADR 0375 gives: it moves the user's content to a vendor they did
  not pick.
- **Builtins read-only.** Declined by the user: a wrong seed would then need a
  migration to fix.
