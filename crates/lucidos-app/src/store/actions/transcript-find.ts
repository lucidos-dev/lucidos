/**
 * Find in the transcript: the thread pane's find target.
 *
 * The transcript draws only a tail of the thread, and a folded turn draws
 * nothing (`threadWindow.ts`, `Disclosure`). So the count comes from the
 * thread's data: every user message and reply chunk, rendered to text the way
 * the transcript renders it. Typing only counts. A step makes its match drawn:
 * it unfolds the turn, asks the transcript to render down to it, then
 * highlights the occurrence and scrolls to it.
 *
 * Plan: `docs/plans/2026-10-07-find-in-previews-and-transcript.md`.
 */

import { signal } from '@preact/signals';
import {
  MAX_FIND_MATCHES, collectPageText, matchSpans, queryPattern, revealRange, spanToRange, stepIndex,
  type FindPainter, type FindResult,
} from '@lucidos/find';
import type { FindTarget } from './find-bar';
import {
  collapsedInitiators, detailsExpanded, expandExchange, threadMap, toggleInitiatorCollapsed,
} from '../store';
import { computeExchanges } from '../thread-events/exchange-grouping';
import { exchangeKey, exchangeUserMessage, isUserBubbleEvent } from '../thread-events/exchange';
import { exchangeResponseEvents } from '../thread-events/exchange-render';
import type { ThreadState } from '../thread-events/thread-meta';
import { getCollapsedVisibleEvents, headClampApplies, isMeaningfulText } from '../event-rendering';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { ensureWholeThreadLoaded } from './thread-loading';
import { isElementVisible, stopFollowingBottom } from '../../components/chat/scrollState';

/** A turn the transcript is asked to draw, by `exchangeKey`. ThreadView
 *  answers it by moving its render window down to that turn. */
export const transcriptRenderRequest = signal<{ threadId: string; key: string } | null>(null);

/** One searchable stretch of the transcript: a user's message, or one reply
 *  chunk, as the plain text the transcript draws for it. A reply chunk carries
 *  the seq of every streamed piece in it: a side question can split the chunk
 *  in two, and the second half is drawn under a later piece's seq. */
export interface TranscriptPassage {
  key: string;
  userSeq: number;
  /** `needsDetails`: the turn draws this chunk only with the full response
   *  on, since without it a turn shows its last chunk alone. */
  part: { kind: 'user' } | { kind: 'reply'; seqs: number[]; needsDetails: boolean };
  text: string;
}

/** Every passage of the thread, in reading order. Pure over the thread's
 *  data, so it counts history the transcript has not drawn. */
export function transcriptPassages(thread: ThreadState, plainText: (md: string) => string): TranscriptPassage[] {
  const passages: TranscriptPassage[] = [];
  for (const exchange of computeExchanges(thread)) {
    const key = exchangeKey(exchange);
    const { userSeq } = exchange;
    if (isUserBubbleEvent(exchange.userEvent)) {
      const md = exchangeUserMessage(exchange);
      if (md.trim()) passages.push({ key, userSeq, part: { kind: 'user' }, text: plainText(md) });
    }
    const events = exchangeResponseEvents(exchange, false, true);
    const shownWithoutDetails = headClampApplies(events, false) ? null : new Set(getCollapsedVisibleEvents(events));
    for (const event of events) {
      if (event.type !== 'text' || !isMeaningfulText(event) || event.seq === undefined) continue;
      const seqs = (event.pieces ?? [{ seq: event.seq }]).flatMap((p) => (p.seq === undefined ? [] : [p.seq]));
      const needsDetails = shownWithoutDetails !== null && !shownWithoutDetails.has(event);
      passages.push({ key, userSeq, part: { kind: 'reply', seqs, needsDetails }, text: plainText(event.md) });
    }
  }
  return passages;
}

let parseDoc: Document | null = null;

/** A passage's text, read from its rendered Markdown in a document never
 *  shown. Blocks are told apart as the drawn transcript tells them. */
export function markdownText(md: string): string {
  parseDoc ??= document.implementation.createHTMLDocument('');
  parseDoc.body.innerHTML = renderMarkdown(md);
  return collectPageText(parseDoc.body, 'markup').text;
}

/** The thread's drawn turns, in DOM order. */
function drawnTurns(threadId: string): HTMLElement[] {
  const selector = `.chat-exchange[data-thread-id="${CSS.escape(threadId)}"]`;
  return [...document.querySelectorAll<HTMLElement>(selector)].filter(isElementVisible);
}

/** A user's message, drawn as a bubble or as a plain panel (an API caller's
 *  or an agent's message). Never a side question or a card's typed answer. */
const USER_MESSAGE = '.initiator-panel .initiator-body .markdown-content';

/** Drawn, not on its way out: a closing `Disclosure` keeps its body, inert. */
function live(roots: Iterable<Element>): Element[] {
  return [...roots].filter((root) => !root.closest('[inert]'));
}

function rootsIn(turn: Element, passage: TranscriptPassage): Element[] {
  const selector = passage.part.kind === 'user'
    ? USER_MESSAGE
    : passage.part.seqs.map((seq) => `.response-chunk[data-text-seq="${seq}"]`).join(', ');
  return live(turn.querySelectorAll(selector));
}

function passageRoots(threadId: string, passage: TranscriptPassage): Element[] {
  const turn = drawnTurns(threadId).find((el) => el.dataset.userSeq === String(passage.userSeq));
  return turn ? rootsIn(turn, passage) : [];
}

function rangesIn(roots: Element[], pattern: RegExp): Range[] {
  const ranges: Range[] = [];
  for (const root of roots) {
    const page = collectPageText(root);
    for (const span of matchSpans(page.text, pattern, MAX_FIND_MATCHES)) ranges.push(spanToRange(page, span));
  }
  return ranges;
}

function sameRange(a: Range, b: Range): boolean {
  return a.startContainer === b.startContainer && a.startOffset === b.startOffset
    && a.endContainer === b.endContainer && a.endOffset === b.endOffset;
}

/** The first drawn turn at or below the top of the transcript's scroller. */
function topTurnSeq(threadId: string): number | null {
  const scroller = document.querySelector<HTMLElement>('.thread-view .thread-content');
  const top = scroller ? scroller.getBoundingClientRect().top : 0;
  const turn = drawnTurns(threadId).find((el) => el.getBoundingClientRect().bottom > top);
  return turn ? Number(turn.dataset.userSeq) : null;
}

interface Hit { passage: number; k: number }

/** Where the first step lands: the first match at or below the reader's
 *  place going forward, the last one above it going back. */
function entryHit(hits: Hit[], passages: TranscriptPassage[], step: 1 | -1, threadId: string): number {
  const seq = topTurnSeq(threadId);
  const at = seq === null ? -1 : passages.findIndex((p) => p.userSeq === seq);
  if (at < 0) return step > 0 ? 0 : hits.length - 1;
  if (step > 0) {
    const i = hits.findIndex((h) => h.passage >= at);
    return i < 0 ? 0 : i;
  }
  for (let i = hits.length - 1; i >= 0; i--) if (hits[i].passage < at) return i;
  return hits.length - 1;
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

/** How long a step waits for its turn to be drawn. A history fold or a long
 *  turn can take a few frames. */
const DRAW_WAIT_MS = 3000;

async function waitForRoots(threadId: string, passage: TranscriptPassage): Promise<Element[]> {
  const deadline = Date.now() + DRAW_WAIT_MS;
  for (;;) {
    const roots = passageRoots(threadId, passage);
    if (roots.length > 0 || Date.now() >= deadline) return roots;
    await sleep(50);
  }
}

/** Let an unfolding roll finish, so the match is measured where it rests. */
async function rollsSettled(el: Element | null): Promise<void> {
  if (!el || typeof el.getAnimations !== 'function') return;
  const rolls = el.getAnimations({ subtree: true }).map((a) => a.finished.then(() => {}, () => {}));
  await Promise.race([Promise.all(rolls), sleep(1000)]);
}

/** Whether the find bar turned the full response on, which `clear` undoes:
 *  the setting is the reader's, and saved. */
let detailsForcedOn = false;

/** Make a match drawn and bring it into view. Resolves to its range, or null
 *  when the transcript could not draw it or the step was overtaken. */
async function reveal(
  threadId: string,
  passage: TranscriptPassage,
  k: number,
  pattern: RegExp,
  stillWanted: () => boolean,
): Promise<Range | null> {
  expandExchange(threadId, passage.userSeq);
  const initiatorKey = `${threadId}:${passage.userSeq}`;
  if (passage.part.kind === 'user' && collapsedInitiators.value.has(initiatorKey)) {
    toggleInitiatorCollapsed(threadId, passage.userSeq);
  }
  if (passage.part.kind === 'reply' && passage.part.needsDetails && !detailsExpanded.value) {
    detailsExpanded.value = true;
    detailsForcedOn = true;
  }
  transcriptRenderRequest.value = { threadId, key: passage.key };
  // The reader asked for this match, so a streaming turn must not pull them
  // back to the live edge while it is on screen.
  stopFollowingBottom();
  const roots = await waitForRoots(threadId, passage);
  if (roots.length === 0) return null;
  await rollsSettled(roots[0].closest('.chat-exchange'));
  if (!stillWanted()) return null;
  const ranges = rangesIn(roots, pattern);
  const range = ranges[Math.min(k, ranges.length - 1)] ?? null;
  if (range) revealRange(range);
  else roots[0].scrollIntoView({ block: 'center' });
  return range;
}

/** The current match's range when it is drawn, without moving anything. */
function drawnRange(threadId: string, passage: TranscriptPassage, k: number, pattern: RegExp): Range | null {
  const ranges = rangesIn(passageRoots(threadId, passage), pattern);
  return ranges[Math.min(k, ranges.length - 1)] ?? null;
}

/** One find session over one thread. Module-scoped, because the find store
 *  asks for the target afresh on every run. `texts` caches each passage's
 *  plain text by its Markdown: the thread is searched on every keystroke, and
 *  its settled passages do not change. */
let session: {
  threadId: string;
  query: string;
  current: number;
  historyAsked: boolean;
  texts: Map<string, string>;
} | null = null;

const NONE: FindResult = { total: 0, current: 0, capped: false };

/** Whether the thread has anything to search: a real thread, not a draft. */
export function transcriptSearchable(threadId: string): boolean {
  const thread = threadMap.value.get(threadId);
  return !!thread && thread.meta.state !== 'composing';
}

export function transcriptTarget(threadId: string, painter: FindPainter): FindTarget {
  const clear = () => {
    session = null;
    painter.clear();
    if (detailsForcedOn) {
      detailsForcedOn = false;
      detailsExpanded.value = false;
    }
  };

  /** Highlight every counted match the transcript draws, and the current one. */
  function paint(passages: TranscriptPassage[], pattern: RegExp, current: Range | null): void {
    const roots = drawnTurns(threadId).flatMap((turn) => passages
      .filter((p) => String(p.userSeq) === turn.dataset.userSeq)
      .flatMap((p) => rootsIn(turn, p)));
    const ranges = rangesIn(roots, pattern);
    let index = current ? ranges.findIndex((r) => sameRange(r, current)) : -1;
    if (current && index < 0) {
      ranges.push(current);
      index = ranges.length - 1;
    }
    painter.paint(ranges, index);
  }

  async function run(query: string, step?: 1 | -1): Promise<FindResult | null> {
    const pattern = queryPattern(query);
    if (!pattern) {
      clear();
      return NONE;
    }
    if (session?.threadId !== threadId) {
      session = { threadId, query: '', current: -1, historyAsked: false, texts: new Map() };
    }
    const own = session;
    // A close or a newer query ends this run: it must not paint or scroll.
    const stillWanted = () => session === own && own.query === query;
    // The count is the whole thread's, so history not loaded yet is fetched
    // once. The transcript holds the reader in place while it folds in.
    if (!own.historyAsked && threadMap.value.get(threadId)?.hasOlderEvents) {
      own.historyAsked = true;
      await ensureWholeThreadLoaded(threadId);
      if (session !== own) return null;
    }
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;

    const plainText = (md: string) => {
      let text = own.texts.get(md);
      if (text === undefined) own.texts.set(md, (text = markdownText(md)));
      return text;
    };
    const passages = transcriptPassages(thread, plainText);
    const hits: Hit[] = [];
    let capped = false;
    for (const [passage, p] of passages.entries()) {
      const spans = matchSpans(p.text, pattern, MAX_FIND_MATCHES + 1 - hits.length);
      for (let k = 0; k < spans.length; k++) {
        if (hits.length === MAX_FIND_MATCHES) {
          capped = true;
          break;
        }
        hits.push({ passage, k });
      }
      if (capped) break;
    }
    const total = hits.length;
    if (own.query !== query) {
      own.query = query;
      own.current = -1;
    }
    if (total === 0) {
      own.current = -1;
      paint(passages, pattern, null);
      return NONE;
    }
    if (step) {
      own.current = own.current < 0 ? entryHit(hits, passages, step, threadId) : stepIndex(own.current, step, total);
    } else {
      own.current = Math.min(own.current, total - 1);
    }

    let current: Range | null = null;
    if (own.current >= 0) {
      const hit = hits[own.current];
      current = step
        ? await reveal(threadId, passages[hit.passage], hit.k, pattern, stillWanted)
        : drawnRange(threadId, passages[hit.passage], hit.k, pattern);
      if (!stillWanted()) return null;
    }
    paint(passages, pattern, current);
    return { total, current: own.current + 1, capped };
  }

  return { run, clear, unsearchable: 'This thread can’t be searched' };
}
