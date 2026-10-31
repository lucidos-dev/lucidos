/**
 * Find in the transcript: the thread pane's find target.
 *
 * The transcript draws only a tail of the thread, and a folded turn draws
 * nothing (`threadWindow.ts`, `Disclosure`). So the count comes from the
 * thread's data: every text the transcript draws (`transcript-passages.ts`).
 * Typing only counts. A step makes its match drawn: it unfolds what hides it,
 * asks the transcript to render down to it, then highlights the occurrence and
 * scrolls to it.
 *
 * Plans: `docs/plans/2026-10-07-find-in-previews-and-transcript.md` and
 * `docs/plans/2026-10-25-find-all-thread-text.md`.
 */

import {
  MAX_FIND_MATCHES, collectPageText, matchSpans, queryPattern, revealRange, spanToRange, stepIndex,
  type ClearBand, type FindPainter, type FindResult,
} from '@lucidos/find';
import type { FindTarget } from './find-bar';
import {
  collapsedInitiators, detailsExpanded, expandExchange, openSubAgentGroups, stepsExpanded, threadMap,
  toggleInitiatorCollapsed, toggleSubAgentGroup,
} from '../store';
import { reopenSideQuestion, sideQuestionsFor } from '../sideQuestions';
import { cachedText, transcriptPassages, type TranscriptPassage } from './transcript-passages';
import { ensureWholeThreadLoaded } from './thread-loading';
import { markRevealScroll, stopFollowingBottom } from '../../components/chat/scrollState';
import { isElementVisible } from '../../utils/elementVisibility';

import { transcriptRenderRequest } from './transcriptRenderRequest';

/** The thread's drawn turns, in DOM order. */
function drawnTurns(threadId: string): HTMLElement[] {
  const selector = `.chat-exchange[data-thread-id="${CSS.escape(threadId)}"]`;
  return [...document.querySelectorAll<HTMLElement>(selector)].filter(isElementVisible);
}

/** Drawn, not on its way out: a closing `Disclosure` keeps its body, inert. */
function live(roots: Iterable<Element>): Element[] {
  return [...roots].filter((root) => !root.closest('[inert]'));
}

/** Where a passage can draw: its turn, or for a side question the feed. */
function containerOf(passage: TranscriptPassage, turns: HTMLElement[]): Element | null {
  if (passage.where.inFeed) return turns[0]?.closest('.thread-feed') ?? null;
  return turns.find((el) => el.dataset.userSeq === String(passage.userSeq)) ?? null;
}

function rootsIn(container: Element | null, passage: TranscriptPassage): Element[] {
  if (!container) return [];
  const { selector, nth } = passage.where;
  const roots = live(container.querySelectorAll(selector));
  return nth === undefined ? roots : roots.slice(nth, nth + 1);
}

function passageRoots(threadId: string, passage: TranscriptPassage): Element[] {
  return rootsIn(containerOf(passage, drawnTurns(threadId)), passage);
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

function nextFrame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()));
}

/** Let an unfolding roll finish, so the match is measured where it rests. The
 *  step's own unfold renders and starts its roll a frame later, so the wait
 *  begins there. Otherwise a roll not yet started reads as none. */
async function rollsSettled(el: Element | null): Promise<void> {
  await nextFrame();
  await nextFrame();
  if (!el || typeof el.getAnimations !== 'function') return;
  const rolls = el.getAnimations({ subtree: true }).map((a) => a.finished.then(() => {}, () => {}));
  await Promise.race([Promise.all(rolls), sleep(1000)]);
}

/** The reader's saved turn settings the find bar turned on, which `clear`
 *  turns off again. */
const forcedOn = new Set<typeof detailsExpanded>();

function forceOn(setting: typeof detailsExpanded): void {
  if (setting.value) return;
  setting.value = true;
  forcedOn.add(setting);
}

/** Open what hides a passage. Folds the reader owns stay as the step leaves
 *  them; only the saved settings go back on close. */
function unfold(threadId: string, passage: TranscriptPassage): void {
  const { initiator, details, steps, subAgent, sideQuestion } = passage.unfold;
  expandExchange(threadId, passage.userSeq);
  if (initiator && collapsedInitiators.value.has(`${threadId}:${passage.userSeq}`)) {
    toggleInitiatorCollapsed(threadId, passage.userSeq);
  }
  if (details) forceOn(detailsExpanded);
  if (steps) forceOn(stepsExpanded);
  if (subAgent && !openSubAgentGroups.value.has(subAgent)) toggleSubAgentGroup(subAgent);
  if (sideQuestion) reopenSideQuestion(sideQuestion);
}

/** Make a match drawn and bring it into view. Resolves to its range, or null
 *  when the transcript could not draw it or the step was overtaken. */
async function reveal(
  threadId: string,
  passage: TranscriptPassage,
  k: number,
  pattern: RegExp,
  stillWanted: () => boolean,
): Promise<Range | null> {
  unfold(threadId, passage);
  transcriptRenderRequest.value = { threadId, key: passage.key };
  // The reader asked for this match, so a streaming turn must not pull them
  // back to the live edge while it is on screen.
  stopFollowingBottom();
  const roots = await waitForRoots(threadId, passage);
  if (roots.length === 0) return null;
  await rollsSettled(roots[0].closest('.chat-exchange, .side-question-card'));
  if (!stillWanted()) return null;
  const ranges = rangesIn(roots, pattern);
  const range = ranges[Math.min(k, ranges.length - 1)] ?? null;
  const scroller = roots[0].closest<HTMLElement>('.thread-content');
  if (range) revealRange(range, scroller ? clearBand(scroller) : undefined);
  else roots[0].scrollIntoView({ block: 'center' });
  // Marked as ours. Unmarked, the transcript reads the step as the reader
  // scrolling, or as the keyboard moving the page, and undoes it.
  markRevealScroll(scroller);
  return range;
}

/** The part of the transcript nothing floats over. On the phone the find bar
 *  covers its top, under the header, and the prompt covers its bottom. */
export function clearBand(scroller: HTMLElement): ClearBand {
  let { top, bottom } = scroller.getBoundingClientRect();
  const bar = scroller.closest('.thread-view')?.querySelector(':scope > .find-bar-slot');
  if (bar) top = Math.max(top, bar.getBoundingClientRect().bottom);
  const prompt = scroller.closest('.thread-view-clip')?.parentElement?.querySelector(':scope > .prompt-area');
  if (prompt) bottom = Math.min(bottom, prompt.getBoundingClientRect().top);
  return { top, bottom };
}

/** The current match's range when it is drawn, without moving anything. */
function drawnRange(threadId: string, passage: TranscriptPassage, k: number, pattern: RegExp): Range | null {
  const ranges = rangesIn(passageRoots(threadId, passage), pattern);
  return ranges[Math.min(k, ranges.length - 1)] ?? null;
}

/** One find session over one thread. Module-scoped, because the find store
 *  asks for the target afresh on every run. `texts` is the passages' text
 *  cache (`cachedText`). */
let session: {
  threadId: string;
  query: string;
  current: number;
  /** The fetch of the thread's older history, once asked for. */
  history: Promise<boolean> | null;
  texts: Map<string, string>;
} | null = null;

const NONE: FindResult = { total: 0, current: 0, capped: false };

export function transcriptTarget(threadId: string, painter: FindPainter): FindTarget {
  const clear = () => {
    session = null;
    painter.clear();
    for (const setting of forcedOn) setting.value = false;
    forcedOn.clear();
  };

  /** Highlight every counted match the transcript draws, and the current one. */
  function paint(passages: TranscriptPassage[], hits: Hit[], pattern: RegExp, current: Range | null): void {
    const turns = drawnTurns(threadId);
    const matched = [...new Set(hits.map((h) => h.passage))];
    const roots = matched.flatMap((i) => rootsIn(containerOf(passages[i], turns), passages[i]));
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
      session = { threadId, query: '', current: -1, history: null, texts: new Map() };
    }
    const own = session;
    // A close or a newer query ends this run: it must not paint or scroll.
    const stillWanted = () => session === own && own.query === query;
    // The count is the whole thread's, so history not loaded yet is fetched
    // once. The transcript holds the reader in place while it folds in, so
    // every run waits for it: a step scrolled during the fold is put back.
    if (!own.history && threadMap.value.get(threadId)?.hasOlderEvents) {
      own.history = ensureWholeThreadLoaded(threadId);
    }
    if (own.history) {
      await own.history;
      if (session !== own) return null;
    }
    const thread = threadMap.value.get(threadId);
    if (!thread) return null;

    const passages = transcriptPassages(thread, cachedText(own.texts), sideQuestionsFor(threadId));
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
      paint(passages, hits, pattern, null);
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
    paint(passages, hits, pattern, current);
    return { total, current: own.current + 1, capped };
  }

  return { run, clear, unsearchable: 'This thread can’t be searched' };
}
