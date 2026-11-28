import type { ComponentChildren } from 'preact';
import { useState } from 'preact/hooks';
import { plainEventMeaning, plainEventName } from '../../store/thread-events';
import { Disclosure } from '../shared/Disclosure';
import { CheckIcon, ChevronRightIcon, CloseIcon } from '../shared/icons';


export type EventRowKind = 'wait' | 'delivery' | 'child' | 'trigger' | 'held' | 'form' | 'change' | 'resume' | 'session' | 'boundary' | 'cache';

export type EventRowTone = 'live' | 'arrived' | 'good' | 'bad' | 'lapsed' | 'halted' | 'none';

const VERDICT_GLYPH: Partial<Record<EventRowTone, () => preact.JSX.Element>> = {
  good: CheckIcon,
  bad: CloseIcon,
};

export type EventRowFact =
  | EventRowChip
  | { kind: 'text'; text: string }
  | { kind: 'glue'; text: string }
  | { kind: 'node'; node: ComponentChildren };

export interface EventRowChip {
  kind: 'chip';
  name: string;
  label?: string;
  meaning?: string;
  sentenceStart?: boolean;
  note?: string;
  onClick?: (chip: HTMLButtonElement) => void;
  popupOpen?: boolean;
  action?: string;
  pending?: boolean;
  role?: string;
}

export interface EventRowFold {
  label: string;
  pre?: boolean;
  open?: boolean;
  body: ComponentChildren;
}

export interface EventRowProps {
  kind: EventRowKind;
  state?: string;
  subject: ComponentChildren;
  stateLabel?: string;
  tone?: EventRowTone;
  detail?: ComponentChildren;
  time?: { label: string; iso: string };
  facts?: (EventRowFact | null | undefined | false)[];
  fold?: EventRowFold;
  actions?: ComponentChildren;
  heldNote?: string;
  role?: string;
  findRow?: number;
}

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
  findRow,
}: EventRowProps) {
  const shown = (facts ?? []).filter((f): f is EventRowFact => !!f);
  const Glyph = VERDICT_GLYPH[tone];
  const card = (
    <div class="event-row" data-role={role} data-kind={kind} data-state={state} data-held={heldNote ? '' : undefined} data-find-row={findRow}>
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
  if (!time) return card;
  return (
    <>
      <time class="event-row-time" dateTime={time.iso}>{time.label}</time>
      {card}
    </>
  );
}

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

export function eventNameChip(chip: EventRowChip, key?: string): ComponentChildren {
  const plain = chip.label ?? plainEventName(chip.name);
  const text = chip.sentenceStart ? plain.charAt(0).toUpperCase() + plain.slice(1) : plain;
  const note = chip.note && [' ', <span key="note" class="event-name-note">{chip.note}</span>];
  const { onClick } = chip;
  if (!onClick) {
    const meaning = chip.meaning ?? plainEventMeaning(chip.name);
    return <code key={key} class="event-name" data-tooltip={meaning}>{text}{note}</code>;
  }
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
