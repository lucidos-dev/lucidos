# 0757: Every permission card draws one shape: bold question, facts, verbatim payload, then the agent's reason in a raised box

- **Status**: Accepted
- **Date**: 2026-11-21

## Context

Sixteen builders wrote permission card text, each in its own style. Some opened
with a fenced code block and a copy button. Some showed a raw trigger id. Some
put the agent's reason first, some last, and some mixed it into the question.
The frontend cards for Claude Code, Codex, the command guard and MCP used their
own classes and wording ("wants to", "Allow?").

The owner reviewed the trigger grant card on a phone, picked a layout and a
reason box, and asked that every permission card follow it.

## Decision

Every permission card, engine-worded or frontend-worded, draws four parts in
this order:

1. **The question**, bold and first. It names who acts and on what: "Let the
   coding agent run this command?".
2. **The facts.** Several exact items are a bullet list of inline code. One item
   sits inline in the question. A consequence the owner must know is a plain
   paragraph.
3. **An exact payload** (a command, a JSON request, a log manifest) as a code
   block. It wraps, caps its height and scrolls vertically. It has no copy
   button.
4. **The agent's reason**, when there is one, in a quote under the heading
   "The agent's reason". The CSS draws it as a raised box on `--bg-tertiary`.

The engine writes this through one builder, `PermissionCardText` in
`lucidos-core` (`thread_events/permission/request.rs`). The frontend draws the
same markdown classes through `CardShape` in `PermissionCard.tsx`. One CSS block
under `.permission-text` styles both. No card shows a uuid or trigger id.

Trigger cards also offer **Always deny**, scoped to that trigger only. The
asked-ahead trigger grant card says **Cancel**, because no act waits on it.

## Rationale

- **One look teaches one habit.** The owner reads the question, checks the
  facts, then weighs the reason. A card that reorders these makes the owner hunt.
- **The reason stays visibly the agent's.** It sits inside the quote on every
  line, so agent text cannot pose as engine text or as an option. The heading
  says whose words they are.
- **An approval is not a snippet.** A copy button invites reuse of text the
  owner is meant to judge. Wrapping and a height cap keep the whole payload
  readable without pushing the buttons off a phone screen.
- **Shared classes, not a shared component.** Engine cards arrive as markdown,
  so the frontend renders to the same classes the markdown engine produces.
  Then one CSS block draws every card, old or new.

## Consequences

- Stored cards keep their old text. The new CSS still draws their quoted reason
  in the raised box, without the heading.
- `REASON_HEADING` exists in Rust and TypeScript. A pin test reads the Rust
  constant, so the two cannot drift.
- A new permission card must use `PermissionCardText` (engine) or `CardShape`
  (frontend). A hand-written body breaks the shape.
- What each card grants is unchanged. Only the wording and the trigger Always
  deny changed.

## Alternatives considered

- **Keep the per-card styles, fix only the trigger card.** Rejected: the owner
  asked for every permission to match, and drift was the original problem.
- **Generate `REASON_HEADING` through codegen.** Rejected as too heavy for one
  string. The pin test gives the same guarantee.
- **A left-stripe quote for the reason.** Rejected: `.claude/rules/frontend-css.md`
  bans the left stripe. A raised box on `--bg-tertiary` replaces it.
- **Always deny as a workspace-wide standing deny on trigger cards.** Rejected:
  the owner wanted the deny in the trigger's context. The command card's Always
  deny stays the workspace-wide one.
