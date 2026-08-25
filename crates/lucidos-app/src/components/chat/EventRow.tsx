import type { ComponentChildren } from 'preact';

/** **The event row**: one transcript marker for everything that arrives from
 *  outside the thread, and for what the thread is waiting on.
 *
 *  Four kinds share it, because they all answer the same question ("what
 *  happened outside this thread") and used to answer it in four dialects: an
 *  *event wait* (armed, matched, expired, stood down), an *event delivery*, a
 *  *child thread* callback, and a *trigger* fire. Between
 *  them they carried four glyph vocabularies, three disclosure labels, and the
 *  event type as an accent chip in one place and prose in another. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`.
 *
 *  The shape is a card: `mark + subject + state` on one line, then the facts,
 *  then an optional fold. The same markup renders in both positions the family
 *  occupies, a response body (the wait) and an initiator panel's `details` (the
 *  other three), which is what lets one primitive serve all four.
 *
 *  It was unboxed until 2026-08-10, on the reasoning that a box is what an
 *  inline AFFORDANCE earns. In the transcript that read as three loose lines of
 *  debris between the step list and the prose, so it is contained now. The
 *  ranking survives instead of the rule: `.step-note-card` (the checkpoint's
 *  Undo) still outweighs this, so a record never looks like something you can
 *  act on.
 *
 *  **It is NOT a step**, and the whole point of the file is that it stops
 *  looking like one. The event wait used to render as `.inline-step` with a
 *  `.step-icon`, which put a green success check on a subscription that might
 *  sleep for hours and ellipsized both the reason and the subscription, the two
 *  things the row exists to show. A marker is a record of a fact, never a
 *  pass/fail verdict, so nothing here takes a step outcome class. */

/** Which of the four surfaces this is. Carried as `data-kind` for tests and
 *  for any kind-specific CSS; the row's LOOK never branches on it, which is
 *  what keeps the four coherent. */
export type EventRowKind = 'wait' | 'delivery' | 'child' | 'trigger';

/** What the mark column says. Deliberately not the kind: the mark answers "did
 *  something arrive", which is the one question all four kinds share, so a
 *  matched wait and a trigger fire get the same glyph and are told apart by
 *  their subject and their state word. */
export type EventRowMark = 'pending' | 'arrived' | 'returned';

/** Monochrome and universally available, on purpose. An emoji here would render
 *  in colour and be the one loud thing in a column of muted marks, which is the
 *  step-icon mistake in a different hat. It said "the ⏰ the trigger panel uses
 *  for its chip" until 2026-08-13, when that chip's emoji became a drawn glyph
 *  along with the last six in host chrome; the colour argument was never what
 *  kept these three as TEXT, which is the actual claim. They are marks in a
 *  column of prose, sized and coloured by the type around them, where an icon
 *  would need a box. */
const EVENT_ROW_MARK: Record<EventRowMark, string> = {
  pending: '○',
  arrived: '↓',
  returned: '↵',
};

/** How the state WORD is tinted. The tint groups the word, it never replaces
 *  it: every state is legible as text, so the row survives a colourblind reader
 *  and reads correctly to a screen reader. */
export type EventRowTone = 'live' | 'arrived' | 'good' | 'bad' | 'lapsed' | 'halted' | 'none';

/** One item on the facts line. `chip` is the shared event-type atom, and it is
 *  the ONLY way an event type is spelled anywhere in the transcript. `glue` is a
 *  connecting word between two chips ("or"), and is the one item the separator
 *  skips on both sides.
 *
 *  There is no `link` kind. There was one, and both its users were a "Go to
 *  event" sitting next to the chip naming the very event it went to: two things
 *  to read for one destination, and one of them repeating nothing. The chip
 *  carries the jump itself now (see `EventRowChip.onClick`). */
export type EventRowFact =
  | EventRowChip
  | { kind: 'text'; text: string }
  | { kind: 'glue'; text: string };

/** The event-type chip, optionally the row's jump. */
export interface EventRowChip {
  kind: 'chip';
  name: string;
  /** Makes the chip ITSELF the link to the event it names. Absent whenever the
   *  event has nowhere to open, which is what keeps a dead tap unreachable
   *  rather than merely unlikely (see `eventHasTarget`). */
  onClick?: () => void;
  /** What pressing it does, as the accessible name and the tooltip. Defaults to
   *  the jump, which is what a chip has meant since the "Go to event" link was
   *  folded into it. A subscription chip opens its condition instead, and a chip
   *  that says only the event type would leave both taps unlabelled. */
  action?: string;
  /** The jump is in flight. The chip keeps its name (it is a fact about the
   *  row, not a button label) and goes inert, so an impatient second tap cannot
   *  start a second navigation. */
  pending?: boolean;
  role?: string;
}

export interface EventRowFold {
  /** Named for its content: `Payload`, `Summary`, `Prompt`. */
  label: string;
  /** Machine data of unknown width (a JSON payload, a sha) gets a `<pre>` that
   *  scrolls rather than wraps, since a wrapped sha reads as two shas. Prose
   *  gets an ordinary block. */
  pre?: boolean;
  body: ComponentChildren;
}

export interface EventRowProps {
  kind: EventRowKind;
  mark: EventRowMark;
  /** Carried as `data-state` for tests and deep links. */
  state?: string;
  /** The sentence the row is about. WRAPS: it is the reason the row exists, and
   *  every kind's subject is something a human or a model wrote. Takes children
   *  rather than a string so a kind can embed a chip or a thread link in it. */
  subject: ComponentChildren;
  /** The state as a word. Omitted only when the row has no state to report. */
  stateLabel?: string;
  tone?: EventRowTone;
  /** Falsy entries are dropped, so a caller can inline a condition rather than
   *  building the array up imperatively. Nothing is invented to fill a gap: a
   *  fact the event does not carry is simply absent, and its separator with it. */
  facts?: (EventRowFact | null | undefined | false)[];
  fold?: EventRowFold;
  /** `data-role`, for the tests and for e2e selectors. */
  role?: string;
}

/** The row's markup, hookless so it stays a pure function of its state. Any
 *  caller needing a hook (the wait's jump tracks its in-flight click) owns it in
 *  a thin wrapper and passes the result down. The split is also what makes this
 *  testable: there is no jsdom in the test infra, so a component carrying a hook
 *  cannot be invoked as a plain function and the tests drive this instead. */
export function eventRowBody({
  kind,
  mark,
  state,
  subject,
  stateLabel,
  tone = 'none',
  facts,
  fold,
  role,
}: EventRowProps) {
  const shown = (facts ?? []).filter((f): f is EventRowFact => !!f);
  return (
    <div class="event-row" data-role={role} data-kind={kind} data-state={state}>
      {/* The subject and its verdict share the top line, so the card opens with
          one readable sentence and the state sits where the eye already is.
          They were stacked, which spent a whole line on a single word and made
          three loose lines out of what is one fact. */}
      <div class="event-row-head">
        <span class="event-row-mark" aria-hidden="true">{EVENT_ROW_MARK[mark]}</span>
        <div class="event-row-subject">{subject}</div>
        {stateLabel && (
          <span class="event-row-state" data-tone={tone}>{stateLabel}</span>
        )}
      </div>
      {shown.length > 0 && <div class="event-row-meta">{renderFacts(shown)}</div>}
      {fold && (
        <details class="event-row-fold">
          <summary>{fold.label}</summary>
          {fold.pre
            ? <pre>{fold.body}</pre>
            : <div class="event-row-fold-body">{fold.body}</div>}
        </details>
      )}
    </div>
  );
}

/** The facts line, flattened rather than mapped, because each item may be
 *  preceded by a separator and a keyed JSX fragment cannot wrap the pair.
 *
 *  A middot goes between adjacent facts, and never touches a `glue`:
 *  "ChangeProposed or ChangeApplied" is one fact expressed as three items, not
 *  three facts. Nothing precedes the first fact, since the state pill left this
 *  line for the header. */
function renderFacts(facts: EventRowFact[]): ComponentChildren[] {
  const out: ComponentChildren[] = [];
  facts.forEach((fact, i) => {
    if (i > 0 && fact.kind !== 'glue' && facts[i - 1].kind !== 'glue') {
      out.push(<span key={`s${i}`} class="event-row-sep" aria-hidden="true">{'·'}</span>);
    }
    out.push(renderFact(fact, i));
  });
  return out;
}

function renderFact(fact: EventRowFact, i: number): ComponentChildren {
  switch (fact.kind) {
    case 'chip':
      return eventNameChip(fact, `c${i}`);
    case 'glue':
      return <span key={`g${i}`} class="event-row-glue">{fact.text}</span>;
    case 'text':
      return <span key={`t${i}`}>{fact.text}</span>;
  }
}

/** The event-type atom, in both of its forms.
 *
 *  Exported because a chip is not always a FACT: the delivery card puts one in
 *  its subject ("Event arrived: `CodingAgentIdled`"), and one event type must
 *  not be spelled two ways depending on which line of the card it landed on.
 *
 *  A plain function rather than a component, matching `eventRowBody` and for the
 *  same reason: there is no jsdom in the test infra, so the tests walk the vnode
 *  tree these return, and a component vnode is opaque to that walk.
 *
 *  With `onClick` it is a real `<button>`, not a `<code>` carrying a handler, so
 *  it is reachable by keyboard and announces itself. Its accessible name says
 *  what pressing it does, because the visible text is a bare event type and says
 *  only what the event IS. */
export function eventNameChip(chip: EventRowChip, key?: string): ComponentChildren {
  if (!chip.onClick) return <code key={key} class="event-name">{chip.name}</code>;
  const label = chip.action ?? `Go to the ${chip.name} event`;
  return (
    <button
      key={key}
      type="button"
      class="event-name event-name-link"
      data-role={chip.role}
      aria-label={label}
      data-tooltip={label}
      aria-busy={chip.pending ? 'true' : undefined}
      disabled={!!chip.pending}
      onClick={chip.onClick}
    >
      {chip.name}
    </button>
  );
}
