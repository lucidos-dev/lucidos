# 0347: Side questions have no slash command: /btw is ordinary text, and an empty box starts side-question mode instead

- **Status**: Accepted
- **Date**: 2026-10-02

## Context

Side questions started as a mirror of Claude Code's terminal `/btw` (ADR 0318).
Since ADR 0324 they no longer use Claude Code's request at all: a Claude Code
thread asks a copy of its session, and a Lucidos Agent thread asks its own
model. Only the name was left.

The name still leaked into the composer. A hold on Stop, the Side question
shortcut over an empty box, and the command menu's `/btw` entry each wrote
`/btw ` into the box. The only way out of that state was to delete the text.
The user found it on screen and asked to drop `/btw` and keep "side question".

## Decision

Nothing treats `/btw` as special. The composer does not parse it, the command
menu does not list it, and the chat route sends it as an ordinary message. Over
an empty box, the hold on Stop and the shortcut turn on *side-question mode*
instead. The mode is a pill above the composer, kept in the draft's compose
selection.

## Rationale

1. **A hidden alias teaches nothing.** With no menu entry and no prefix in the
   box, only someone who already knew could type `/btw`. The user ruled it out.
2. **A mode can be left.** The pill's × and Escape turn it off and keep the
   text. A prefix could only be deleted by hand.
3. **One name for one feature.** The UI says "Side question" everywhere, and
   the borrowed terminal word no longer competes with it.
4. **The compose selection already persists per draft.** The mode rides the
   existing compose PUT and SSE, so it survives a reload and reaches every
   device. The engine stores that JSON without reading it.

## Consequences

- The chat route's 400 with reason `side-question` is gone. A caller that
  posts `/btw …` starts an ordinary turn.
- A Claude Code thread receives typed `/btw` text as a prompt. Headless Claude
  Code registers no such command, so it reads as text.
- The mode is turned off as an explicit `sideQuestionMode: false`, since the
  compose PUT omits an empty selection and the engine would keep the old one.

## Alternatives considered

- **Keep typed `/btw` as a hidden alias that turns the mode on.** Cheap, but
  nothing could teach it, and the user dropped it.
- **Drop the empty-box path altogether.** Side questions would start only from
  a typed draft. Rejected: asking during a running turn from an empty box is
  the common case.
- **Keep the mode in client memory.** Simpler, but a reload would drop the pill
  and leave a side question to be sent as a turn.
