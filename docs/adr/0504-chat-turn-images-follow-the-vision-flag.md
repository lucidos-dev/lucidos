# 0504: A chat turn withholds images from a text-only row, and a model with no row keeps them

- **Status**: Accepted
- **Date**: 2026-10-17

## Context

ADR 0379 gave each `models` row a *vision flag*, but only image description
read it. A chat turn sent images to any model. The OpenAI Responses wire
dropped every image, which hid the gap until a fix made it send them. A
text-only model then got images and could answer 400, on every later turn too,
since history images are re-sent.

The builtin `gpt-5.3-codex-spark` on direct OpenAI is the concrete case. Chat
Completions had the same hole for its text-only rows. Phases and evidence:
`docs/plans/2026-10-17-chat-turn-images-follow-the-vision-flag.md`.

## Decision

`RoutingProvider::chat` reads the vision flag of the model the call is sent as.
A row with the flag off gets a short note in place of each image. A model with
no registry row keeps its images, and its provider decides.

## Rationale

- **One chokepoint.** Three producers add images: the chat turn's attachments
  and history, tool results, and special-tool answers. The router sees all of
  them and already holds the registry. A gate per producer would miss the next
  producer.
- **The model sent, not the model named.** A retired id is sent as its
  successor (ADR 0418), so the successor's flag is the one that matters.
- **A note, not silence.** The note names the model and the switch to flip, so
  the model can tell the user why it cannot see the picture. The history text
  still carries any image description.
- **No row means no flag, so no gate.** The flag is a fact about a row. An
  env-only id, or a deleted row still saved as a thread's model, has no fact to
  read. It also has no switch the note could point at. Passing its images keeps
  today's behaviour, so the gate regresses nothing.
- **Why ADR 0379 answers no for the same model.** Image description chooses
  where to send images among alternatives, so "unknown" rightly loses there. A
  chat turn has one model, which the user picked.

## Consequences

- A model whose flag is off stops seeing images in chat. That includes every
  user-added row until its flag is switched on, since a new row starts off.
- `model_registry` has two readers: `vision_flag` returns `Option<bool>` for
  the chat gate, and `reads_images` is `vision_flag == Some(true)`, for image
  description.
- The context budget still counts a withheld image at its image estimate. That
  over-counts, which only trims earlier.
- Claude Code threads never reach `RoutingProvider`, so they are untouched.

## Alternatives considered

- **Strip images for a model with no row too**, by gating on `reads_images`.
  Lost: it changes behaviour for ids nobody declared anything about, and the
  note could not name a fix. Offered to the user as a fork, and declined.
- **Gate at each producer.** Lost: three sites to keep in step, plus any added
  later.
- **Guess vision from the model id.** Lost for the reason ADR 0379 gives: ids
  drift, and a wrong guess is the failure the flag exists to stop.
- **Refuse the turn.** Lost: the user's text still has an answer, and a refusal
  on every later turn would strand the thread on its history images.
