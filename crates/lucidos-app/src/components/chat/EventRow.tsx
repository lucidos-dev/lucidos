import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';
import { plainEventMeaning, plainEventName } from '../../store/thread-events';
import { Disclosure } from '../shared/Disclosure';
import { CheckIcon, ChevronRightIcon, CloseIcon } from '../shared/icons';

/** **The event row**: one transcript marker for everything that arrives from
 *  outside the thread, and for what the thread is waiting on.
 *
 *  Four kinds share it, because they all answer the same question ("what
 *  happened outside this thread") and used to answer it in four dialects: an
 *  *event wait* (waiting, done, gave up, stopped), an *event delivery*, a
 *  *child thread* callback, and a *trigger* fire. Between
 *  them they carried four glyph vocabularies, three disclosure labels, and the
 *  event type as an accent chip in one place and prose in another. See
 *  `docs/plans/2026-08-10-one-event-row-for-the-transcript.md`.
 *
 *  The shape is a card: `subject + state` on one line, then the facts,
 *  then an optional fold. The same markup renders in both positions the family
 *  occupies, a response body (the wait) and an initiator panel's `details` (the
 *  other three), which is what lets one primitive serve all four.
 *
 *  It was unboxed until 2026-08-10, on the reasoning that a box is what an
 *  inline AFFORDANCE earns. In the transcript that read as three loose lines of
 *  debris between the step list and the prose, so it is contained now. The
 *  ranking survives instead of the rule: `.step-note-card` (the checkpoint's
 *  Undo) still outweighs this, so a record never looks like something you can
 *  act on. Two exceptions carry `actions` at the same weight: an open *form
 *  request*'s Open (ADR 0275), and a change's Diff and Revert.
 *
 *  No glyph leads the subject: the subject and the state word carry the row.
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
export type EventRowKind = 'wait' | 'delivery' | 'child' | 'trigger' | 'held' | 'form' | 'change' | 'resume' | 'boundary';

/** How the state WORD is tinted. The tint groups the word, it never replaces
 *  it: every state is legible as text, so the row survives a colourblind reader
 *  and reads correctly to a screen reader. */
export type EventRowTone = 'live' | 'arrived' | 'good' | 'bad' | 'lapsed' | 'halted' | 'none';

/** The glyph before the state word. Only a pass or a fail earns one: every
 *  other tone is a state rather than a verdict, and a mark on it would read as
 *  a result the row never had. */
const VERDICT_GLYPH: Partial<Record<EventRowTone, () => preact.JSX.Element>> = {
  good: CheckIcon,
  bad: CloseIcon,
};

/** One item on the facts line. `chip` is the shared event-type atom, and it is
 *  the ONLY way an event type is spelled anywhere in the transcript. `glue` is a
 *  connecting word around the chips ("watching for", "or"), and is the one item the separator
 *  skips on both sides.
 *
 *  There is no `link` kind. There was one, and both its users were a "Go to
 *  event" sitting next to the chip naming the very event it went to: two things
 *  to read for one destination, and one of them repeating nothing. The chip
 *  carries the jump itself now (see `EventRowChip.onClick`). */
export type EventRowFact =
  | EventRowChip
  | { kind: 'text'; text: string }
  | { kind: 'glue'; text: string }
  /** Markup the row cannot spell as text, such as a link to a thread. */
  | { kind: 'node'; node: ComponentChildren };

/** The event-type chip, optionally the row's jump. */
export interface EventRowChip {
  kind: 'chip';
  /** The raw event type. The chip shows it in plain words (`plainEventName`),
   *  and its tooltip says what the event means (`plainEventMeaning`). */
  name: string;
  /** Words for the chip in place of the event's plain name, when the event's
   *  own reason says more (a hardening reminder reads "hardening needed"). */
  label?: string;
  /** The tooltip in place of the event type's meaning, for a chip whose
   *  `label` names something more specific than the type. */
  meaning?: string;
  /** The chip opens a sentence, so its plain name takes a capital. */
  sentenceStart?: boolean;
  /** A quiet qualifier after the name ("with a condition", "6 conditions"). Styled
   *  apart from the name so the event type still reads as one token. */
  note?: string;
  /** Makes the chip ITSELF the link to the event it names. Absent whenever the
   *  event has nowhere to open, which is what keeps a dead tap unreachable
   *  rather than merely unlikely (see `eventHasTarget`). It receives the chip,
   *  for a press that opens a popover at it. */
  onClick?: (chip: HTMLButtonElement) => void;
  /** Set when the press opens a popover at the chip: whether it is open now.
   *  Absent for a jump, which opens nothing. */
  popupOpen?: boolean;
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
  /** Named for its content: `Details`, `Summary`, `Prompt`. */
  label: string;
  /** Machine data of unknown width (a JSON payload, a sha) gets a `<pre>` that
   *  scrolls rather than wraps, since a wrapped sha reads as two shas. Prose
   *  gets an ordinary block. */
  pre?: boolean;
  /** Starts unfolded. Only for a body the reader must not miss, such as the
   *  error behind a failed apply. */
  open?: boolean;
  body: ComponentChildren;
}

export interface EventRowProps {
  kind: EventRowKind;
  /** Carried as `data-state` for tests and deep links. */
  state?: string;
  /** The sentence the row is about. WRAPS: it is the reason the row exists, and
   *  every kind's subject is something a human or a model wrote. Takes children
   *  rather than a string so a kind can embed a chip or a thread link in it. */
  subject: ComponentChildren;
  /** The state as a word. Omitted only when the row has no state to report. */
  stateLabel?: string;
  tone?: EventRowTone;
  /** A sentence under the head line, at full weight, for a subject that is a
   *  chip. The wait's pill says "Waiting for", and this says what. */
  detail?: ComponentChildren;
  /** When the row's event was recorded, already formatted, and its ISO source.
   *  Drawn above the card. Only a row inside a response body needs one: a row
   *  in an initiator panel sits under that panel's own timestamp. */
  time?: { label: string; iso: string };
  /** Falsy entries are dropped, so a caller can inline a condition rather than
   *  building the array up imperatively. Nothing is invented to fill a gap: a
   *  fact the event does not carry is simply absent, and its separator with it. */
  facts?: (EventRowFact | null | undefined | false)[];
  fold?: EventRowFold;
  /** What the reader can do from the row: an open form request's Open, a
   *  change's Diff and Revert. A plain record carries none. */
  actions?: ComponentChildren;
  /** Set while the row waits behind an open question: the card dims and
   *  carries this line. Absent on every row that is not held. */
  heldNote?: string;
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
  state,
  subject,
  stateLabel,
  tone = 'none',
  detail,
  time,
  facts,
  fold,
  actions,
  heldNote,
  role,
}: EventRowProps) {
  const shown = (facts ?? []).filter((f): f is EventRowFact => !!f);
  const Glyph = VERDICT_GLYPH[tone];
  const card = (
    <div class="event-row" data-role={role} data-kind={kind} data-state={state} data-held={heldNote ? '' : undefined}>
      {/* The subject and its verdict share the top line, so the card opens with
          one readable sentence and the state sits where the eye already is.
          They were stacked, which spent a whole line on a single word and made
          three loose lines out of what is one fact. */}
      <div class="event-row-head">
        <div class="event-row-subject">{subject}</div>
        {stateLabel && (
          <span class="event-row-state" data-tone={tone}>
            {Glyph && <span class="event-row-state-glyph" aria-hidden="true"><Glyph /></span>}
            {stateLabel}
          </span>
        )}
      </div>
      {detail && <div class="event-row-detail">{detail}</div>}
      {shown.length > 0 && <div class="event-row-meta">{renderFacts(shown)}</div>}
      {heldNote && <div class="event-row-held-note">{heldNote}</div>}
      {fold && <EventRowFoldView {...fold} />}
      {actions && <div class="event-row-actions">{actions}</div>}
    </div>
  );
  // The stamp sits above the card, right-aligned, as a user bubble carries its
  // own. Every timestamp in the transcript then reads from the same place.
  if (!time) return card;
  return (
    <>
      <time class="event-row-time" dateTime={time.iso}>{time.label}</time>
      {card}
    </>
  );
}

/** The row's fold. Its own component because the open state needs a hook,
 *  and `eventRowBody` stays hookless. */
export function EventRowFoldView({ label, pre, open: startsOpen = false, body }: EventRowFold) {
  const [open, setOpen] = useState(startsOpen);
  return (
    <div class="event-row-fold">
      <button
        type="button"
        class="event-row-fold-toggle"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span class="event-row-fold-chevron" aria-hidden="true">
          <ChevronRightIcon size="1em" />
        </span>
        {label}
      </button>
      <Disclosure open={open}>
        {pre
          ? <pre class="event-row-fold-pre">{body}</pre>
          : <div class="event-row-fold-body">{body}</div>}
      </Disclosure>
    </div>
  );
}

/** The facts line, flattened rather than mapped, because each item may be
 *  preceded by a separator and a keyed JSX fragment cannot wrap the pair.
 *
 *  A middot goes between adjacent facts, and never touches a `glue`:
 *  "ChangeProposed or ChangeApplied" is one fact expressed as three items, not
 *  three facts. Nothing precedes the first fact, since the state word left this
 *  line for the header.
 *
 *  A middot travels with the fact after it, so a wrapped line never ends on a
 *  stranded separator. */
function renderFacts(facts: EventRowFact[]): ComponentChildren[] {
  return facts.map((fact, i) => {
    if (i === 0 || fact.kind === 'glue' || facts[i - 1].kind === 'glue') return renderFact(fact, i);
    return (
      <span key={`f${i}`} class="event-row-fact">
        <span class="event-row-sep" aria-hidden="true">{'·'}</span>
        {renderFact(fact, i)}
      </span>
    );
  });
}

function renderFact(fact: EventRowFact, i: number): ComponentChildren {
  switch (fact.kind) {
    case 'chip':
      return eventNameChip(fact, `c${i}`);
    case 'glue':
      return <span key={`g${i}`} class="event-row-glue">{fact.text}</span>;
    case 'text':
      return <span key={`t${i}`}>{fact.text}</span>;
    case 'node':
      return <span key={`n${i}`}>{fact.node}</span>;
  }
}

/** The event-type atom, in both of its forms.
 *
 *  Exported because a chip is not always a FACT: the delivery card's subject IS
 *  one ("Coding agent stopped working"), and one event type must
 *  not be spelled two ways depending on which line of the card it landed on.
 *
 *  A plain function rather than a component, matching `eventRowBody` and for the
 *  same reason: there is no jsdom in the test infra, so the tests walk the vnode
 *  tree these return, and a component vnode is opaque to that walk.
 *
 *  The visible text is the plain name, and the tooltip says what it means. The
 *  raw type is not on the chip. A turn's route popover lists it under Technical
 *  details, with the types a re-entry's wait watched, and a filtered watch's
 *  condition view names it too.
 *
 *  With `onClick` it is a real `<button>`, not a `<code>` carrying a handler, so
 *  it is reachable by keyboard and announces itself. Its accessible name says
 *  what pressing it does, because the visible text says only what the event IS. */
export function eventNameChip(chip: EventRowChip, key?: string): ComponentChildren {
  const plain = chip.label ?? plainEventName(chip.name);
  const text = chip.sentenceStart ? plain.charAt(0).toUpperCase() + plain.slice(1) : plain;
  // The space is real text so a copied chip reads "coding agent stopped working 6 conditions".
  const note = chip.note && [' ', <span key="note" class="event-name-note">{chip.note}</span>];
  const { onClick } = chip;
  if (!onClick) {
    const meaning = chip.meaning ?? plainEventMeaning(chip.name);
    return <code key={key} class="event-name" data-tooltip={meaning}>{text}{note}</code>;
  }
  // Starts with the visible words, so a voice user can say what they see.
  const label = chip.action ?? `${text} · go to the event`;
  return (
    <button
      key={key}
      type="button"
      class="event-name event-name-link"
      data-role={chip.role}
      aria-label={label}
      data-tooltip={label}
      aria-haspopup={chip.popupOpen === undefined ? undefined : 'dialog'}
      aria-expanded={chip.popupOpen}
      aria-busy={chip.pending ? 'true' : undefined}
      disabled={!!chip.pending}
      onClick={(e) => onClick(e.currentTarget)}
    >
      {text}
      {note}
    </button>
  );
}
