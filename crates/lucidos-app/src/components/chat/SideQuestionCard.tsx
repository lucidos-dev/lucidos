import type { VNode } from 'preact';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { dismissSideQuestion, reopenSideQuestion, type SideQuestion } from '../../store/sideQuestions';
import type { BodyRow, BodySection } from '../../store/event-rendering';
import { exchangeKey, type Exchange } from '../../store/thread-events/exchange';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { appsList } from '../../store/store';
import { loadedOr } from '../../store/types';
import { Disclosure } from '../shared/Disclosure';
import { handleMarkdownLinkClick } from '../shared/markdownLinkClick';
import { ChevronDownIcon } from '../shared/icons';
import { MarkdownBlock, UserImages } from './chat-exchange-parts';

/** One shared empty list, so a turn without cards keeps a stable prop. */
export const NO_SIDE_QUESTIONS: readonly SideQuestion[] = [];

/** Said on every card, so nobody mistakes the answer for part of the thread. */
export const SIDE_QUESTION_NOTE = 'Not added to the conversation';

/** How long a pending card waits before it says "Thinking". A live answer
 *  often lands sooner, and a flash of the word reads as a glitch. */
export const SIDE_QUESTION_THINKING_DELAY_MS = 600;

/** The live step row's own "Thinking" label and shimmer, so a pending side
 *  question reads as working exactly the way a turn does. */
function Thinking() {
  const shown = useDelayedFlag(true, SIDE_QUESTION_THINKING_DELAY_MS);
  return (
    <div class="side-question-thinking" aria-live="polite">
      {shown ? <span class="running-shimmer">Thinking</span> : null}
    </div>
  );
}

/** One card whose head folds it to a line at its moment and unfolds it
 *  again, with the disclosure roll. Folding records the dismissal; unfolding
 *  a folded card shows it again on this device only. */
export function SideQuestionCard({ item }: { item: SideQuestion }) {
  const open = !item.dismissed;
  return (
    <div
      class="side-question-card"
      data-role="side-question-card"
      data-side-question-id={item.id}
      data-status={item.status}
      data-collapsed={open ? undefined : ''}
    >
      <button
        type="button"
        class="side-question-head"
        aria-expanded={open}
        aria-label={`Side question: ${item.question}`}
        data-tooltip={open ? 'Collapse' : 'Expand'}
        onClick={() => (open ? void dismissSideQuestion(item) : reopenSideQuestion(item.id))}
      >
        <span class="side-question-label">Side question</span>
        <span class="side-question-summary">{open ? SIDE_QUESTION_NOTE : item.question}</span>
        <span class="side-question-chevron" aria-hidden="true"><ChevronDownIcon /></span>
      </button>
      <Disclosure open={open} bodyClass="side-question-body">
        <div class="user-bubble side-question-question">
          {item.question}
          <UserImages imageHashes={item.imageHashes} />
        </div>
        {item.status === 'pending' && <Thinking />}
        {item.status === 'answered' && (
          // The card sits outside the turn body, so it carries the router
          // itself. Without it the webview follows a `repo:` link to the OS.
          <MarkdownBlock
            html={renderMarkdown(item.answer)}
            onClick={(e) => handleMarkdownLinkClick(e, loadedOr(appsList.value, []))}
          />
        )}
        {item.status === 'failed' && (
          <div class="side-question-error" role="alert">{item.error}</div>
        )}
      </Disclosure>
    </div>
  );
}

/** Cards asked at the same point in the thread, stacked in one box. The box
 *  is what lets it sit between two turns (`childAtLine` in scrollAnchor.ts)
 *  or between two sections of one turn's body. */
export function SideQuestionGroup({ items }: { items: readonly SideQuestion[] }) {
  return (
    <div class="side-questions" data-role="side-questions">
      {items.map((item) => <SideQuestionCard key={item.id} item={item} />)}
    </div>
  );
}

/** The feed's turn nodes with the side questions no turn owns
 *  (`sideQuestionOwners`), each placed before the first node later than its
 *  `afterSeq`. Every later turn draws below it, even past a card pinned out
 *  of order (ADR 0284). A node that is not one exchange (the queued group)
 *  counts as later. A card asked above the render window stays out until the
 *  window walks up to it. */
export function placeSideQuestions(
  nodes: VNode[],
  exchanges: readonly Exchange[],
  items: readonly SideQuestion[],
  windowClipped: boolean,
): VNode[] {
  if (items.length === 0) return nodes;
  const seqByKey = new Map(exchanges.map((ex) => [exchangeKey(ex), ex.userSeq]));
  const seqOf = (node: VNode) => seqByKey.get(String(node.key)) ?? Infinity;
  const groups = new Map<number, SideQuestion[]>();
  for (const item of items) {
    const { afterSeq } = item;
    const later = afterSeq === null ? -1 : nodes.findIndex((node) => seqOf(node) > afterSeq);
    const at = later === -1 ? nodes.length : later;
    if (at === 0 && windowClipped) continue;
    groups.set(at, [...(groups.get(at) ?? []), item]);
  }
  const out: VNode[] = [];
  for (let i = 0; i <= nodes.length; i++) {
    const group = groups.get(i);
    // Keyed by the turn above, which neither a dismissal nor a later turn
    // changes, so a pending card keeps its state instead of remounting.
    const above = i === 0 ? 'start' : String(nodes[i - 1].key);
    if (group) out.push(<SideQuestionGroup key={`side-questions:after:${above}`} items={group} />);
    if (i < nodes.length) out.push(nodes[i]);
  }
  return out;
}

/** The turn each card belongs to: the latest one begun at or before its
 *  `afterSeq` that draws as a turn of its own. A queued message or a folded
 *  restart pause never does. The card then sits inside that turn at its moment
 *  (`placeInBody`). A card with no such turn stays in the feed. */
export function sideQuestionOwners(
  exchanges: readonly Exchange[],
  items: readonly SideQuestion[],
  drawsNoTurn: (index: number) => boolean,
): { owned: Map<number, SideQuestion[]>; unowned: SideQuestion[] } {
  const owned = new Map<number, SideQuestion[]>();
  const unowned: SideQuestion[] = [];
  for (const item of items) {
    const { afterSeq } = item;
    let owner = -1;
    exchanges.forEach((ex, i) => {
      if (afterSeq === null || drawsNoTurn(i) || ex.userSeq > afterSeq) return;
      if (owner === -1 || ex.userSeq > exchanges[owner].userSeq) owner = i;
    });
    if (owner === -1) unowned.push(item);
    else owned.set(owner, [...(owned.get(owner) ?? []), item]);
  }
  return { owned, unowned };
}

/** One piece of a turn's drawn body: a `.response-content` section, or the
 *  side questions asked at that point. */
export type BodyPiece =
  | { kind: 'section'; key: string; rows: BodyRow[] }
  | { kind: 'cards'; key: string; items: SideQuestion[] };

/** A turn's body with its side questions at their moments: each goes before
 *  the first row later than its `afterSeq`, splitting a step run if it must. A
 *  row with no seq (the live Thinking row) counts as later.
 *
 *  A card is a sibling of the sections, never a row inside one, so nothing
 *  counts it as something the agent drew (`DRAWN_ROW_SELECTOR`). It is keyed
 *  by the row above it, which later rows never change. */
export function placeInBody(sections: readonly BodySection[], items: readonly SideQuestion[]): BodyPiece[] {
  let pending = [...items];
  const out: BodyPiece[] = [];
  let above = 'start';
  // The cards a row at `seq` comes after, in asking order.
  const dueBefore = (seq: number | undefined): SideQuestion[] => {
    const due = pending.filter((item) => seq === undefined || seq > (item.afterSeq ?? Infinity));
    if (due.length > 0) pending = pending.filter((item) => !due.includes(item));
    return due;
  };
  for (const section of sections) {
    let key = String(section.key);
    let rows: BodyRow[] = [];
    const cut = (due: SideQuestion[], nextKey: number | string) => {
      if (rows.length > 0) out.push({ kind: 'section', key, rows });
      out.push({ kind: 'cards', key: `side-questions:after:${above}`, items: due });
      key = String(nextKey);
      rows = [];
    };
    for (const row of section.rows) {
      if (row.kind !== 'steps') {
        const due = dueBefore(row.event.seq);
        if (due.length > 0) cut(due, row.key);
        let rest: BodyRow = row;
        let offset = 0;
        for (let split = rest.kind === 'text' ? textSplit(rest, pending, offset) : null; split;
          split = textSplit(split.tail, pending, offset)) {
          const { head, tail, due: splitDue } = split;
          pending = pending.filter((item) => !splitDue.includes(item));
          rows.push(head);
          above = String(head.key);
          cut(splitDue, tail.key);
          offset += head.event.md.length;
          rest = tail;
        }
        rows.push(rest);
        above = String(rest.key);
        continue;
      }
      // A run keeps its own key for its first part, which the clamp may have cut.
      const part = (from: number, to?: number): BodyRow => (
        from === 0 && to === undefined ? row
          : { ...row, key: from === 0 ? row.key : row.steps[from].index, steps: row.steps.slice(from, to) }
      );
      let start = 0;
      row.steps.forEach((step, k) => {
        const due = dueBefore(step.event.seq);
        if (due.length === 0) return;
        if (k > start) {
          rows.push(part(start, k));
          above = String(row.steps[k - 1].index);
        }
        cut(due, step.index);
        start = k;
      });
      rows.push(part(start));
      above = String(row.steps[row.steps.length - 1].index);
    }
    if (rows.length > 0) out.push({ kind: 'section', key, rows });
  }
  if (pending.length > 0) out.push({ kind: 'cards', key: `side-questions:after:${above}`, items: pending });
  return out;
}

type TextRow = Extract<BodyRow, { kind: 'text' }>;

/** Splits a merged text chunk for the earliest of `items` whose question
 *  falls inside it, or null when none does yet. `offset` is where `row`
 *  starts in the chunk it was cut from, so the tail's key never moves. */
function textSplit(row: TextRow, items: readonly SideQuestion[], offset: number):
  { head: TextRow; tail: TextRow; due: SideQuestion[] } | null {
  const { md, pieces } = row.event;
  if (!pieces) return null;
  let at: number | null = null;
  let due: SideQuestion[] = [];
  for (const item of items) {
    const cutAt = paragraphBreakAfter(md, pieces, item.afterSeq ?? Infinity);
    if (cutAt === null || (at !== null && cutAt > at)) continue;
    due = cutAt === at ? [...due, item] : [item];
    at = cutAt;
  }
  if (at === null) return null;
  const split = at;
  const headPieces = pieces.filter((p) => p.at < split);
  // The piece the break falls in, or one starting exactly on it.
  const tailSeq = pieces.filter((p) => p.at <= split).pop()?.seq;
  const tailPieces = [
    { at: 0, seq: tailSeq },
    ...pieces.filter((p) => p.at > split).map((p) => ({ at: p.at - split, seq: p.seq })),
  ];
  const base = String(row.key).split('@')[0];
  return {
    head: { ...row, event: textEvent(md.slice(0, split), headPieces) },
    tail: { ...row, key: `${base}@${offset + split}`, event: textEvent(md.slice(split), tailPieces) },
    due,
  };
}

function textEvent(md: string, pieces: { at: number; seq?: number }[]): TextRow['event'] {
  return { type: 'text', md, seq: pieces[0]?.seq, ...(pieces.length > 1 ? { pieces } : {}) };
}

/** Where a chunk may split for a question asked after `afterSeq`: the first
 *  paragraph break at or after the first piece written later, outside a code
 *  fence and with text after it. A chunk begun after the question is not split
 *  here: the whole row already sits below the card. */
function paragraphBreakAfter(md: string, pieces: readonly { at: number; seq?: number }[], afterSeq: number): number | null {
  const later = pieces.find((p) => p.seq === undefined || p.seq > afterSeq);
  if (!later || later.at === 0) return null;
  for (let from = Math.max(0, later.at - 2); ;) {
    const found = md.indexOf('\n\n', from);
    if (found === -1 || found + 2 >= md.length) return null;
    const fences = md.slice(0, found).match(/^ {0,3}(```|~~~)/gm)?.length ?? 0;
    if (fences % 2 === 0) return found + 2;
    from = found + 1;
  }
}
