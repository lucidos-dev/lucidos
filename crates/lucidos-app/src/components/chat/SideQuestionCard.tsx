import type { VNode } from 'preact';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { dismissSideQuestion, reopenSideQuestion, retrySideQuestion, type SideQuestion } from '../../store/sideQuestions';
import type { BodyRow, BodySection } from '../../store/event-rendering';
import { exchangeKey, type Exchange } from '../../store/thread-events/exchange';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { appsList } from '../../store/store';
import { loadedOr } from '../../store/types';
import { Disclosure } from '../shared/Disclosure';
import { handleMarkdownLinkClick } from '../shared/markdownLinkClick';
import { ChevronDownIcon, RetryIcon } from '../shared/icons';
import { MarkdownBlock, UserMessageBody } from './chat-exchange-parts';

export const NO_SIDE_QUESTIONS: readonly SideQuestion[] = [];

export const SIDE_QUESTION_NOTE = 'Not added to the conversation';

export const SIDE_QUESTION_THINKING_DELAY_MS = 600;

function Thinking() {
  const shown = useDelayedFlag(true, SIDE_QUESTION_THINKING_DELAY_MS);
  return (
    <div class="side-question-thinking" aria-live="polite">
      {shown ? <span class="running-shimmer">Thinking</span> : null}
    </div>
  );
}

export function SideQuestionCard({ item }: { item: SideQuestion }) {
  const open = !item.dismissed;
  const routeLinks = (e: MouseEvent) => handleMarkdownLinkClick(e, loadedOr(appsList.value, []));
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
        <div class="user-bubble side-question-question" onClick={routeLinks}>
          <UserMessageBody html={renderMarkdown(item.question)} imageHashes={item.imageHashes} />
        </div>
        {item.status === 'pending' && <Thinking />}
        {item.status === 'answered' && (
          <MarkdownBlock html={renderMarkdown(item.answer)} onClick={routeLinks} className="side-question-answer" />
        )}
        {item.status === 'failed' && (
          <div class="side-question-failure">
            <span class="side-question-error" role="alert">{item.error}</span>
            <button type="button" class="side-question-retry" onClick={() => void retrySideQuestion(item)}>
              <RetryIcon />Retry
            </button>
          </div>
        )}
      </Disclosure>
    </div>
  );
}

export function SideQuestionGroup({ items }: { items: readonly SideQuestion[] }) {
  return (
    <div class="side-questions" data-role="side-questions">
      {items.map((item) => <SideQuestionCard key={item.id} item={item} />)}
    </div>
  );
}

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
    const above = i === 0 ? 'start' : String(nodes[i - 1].key);
    if (group) out.push(<SideQuestionGroup key={`side-questions:after:${above}`} items={group} />);
    if (i < nodes.length) out.push(nodes[i]);
  }
  return out;
}

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

export type BodyPiece =
  | { kind: 'section'; key: string; rows: BodyRow[] }
  | { kind: 'cards'; key: string; items: SideQuestion[] };

export function placeInBody(sections: readonly BodySection[], items: readonly SideQuestion[]): BodyPiece[] {
  let pending = [...items];
  const out: BodyPiece[] = [];
  let above = 'start';
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
