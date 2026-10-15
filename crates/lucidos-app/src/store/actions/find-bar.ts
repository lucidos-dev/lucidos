/**
 * The find bar: one bar, many targets.
 *
 * A find session belongs to one surface, the content pane or the thread pane,
 * and searches what that surface shows. A target answers a query however it
 * reaches its text. An app frame is isolated (ADR 0227), so it is asked over
 * the app bridge, and its SDK matches there. The HTML preview is isolated too,
 * and its bridge carries a copy of the same matcher. A text preview is the
 * host's own DOM, so the host runs the matcher (`@lucidos/find`). The transcript
 * counts the thread's data and draws a match on a step (`transcript-find.ts`).
 * The targets live in `find-targets.ts`, loaded on demand; this module decides
 * which one a query goes to.
 *
 * Plans: `docs/plans/2026-10-07-find-in-app.md` and
 * `docs/plans/2026-10-07-find-in-previews-and-transcript.md`.
 */

import { effect, signal } from '@preact/signals';
import type { FindResult } from '@lucidos/find';
import {
  filePreviewEditing, filePreviewSource, findRequest, focusedPane, focusedThreadId, lineScrollTarget, mobileView,
  panelOverlay, threadMap,
} from '../store';
import { MIN_FIND_QUERY_CHARS, findQueryChars } from './find-request';
import { parseRepoPath } from '../repoPath';
import { dataPreviewBody, repoPreviewBody } from '../../components/files/previewBody';
import { lazyModule, whenLoaded, type PendingOpen } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';
import { holdSoftwareKeyboard } from '../../utils/softwareKeyboard';
import { hasCoarsePointer, isMobile } from '../../utils/viewport';

/** The matcher and the targets, fetched once the boot splash lifts (ADR 0436).
 *  The bar opens only once they are in memory, so `run` can always reach them. */
export const findTargets = lazyModule(() => import('./find-targets'));
prefetchWhenIdle(findTargets);

/** Where a find bar lives, and what it searches. */
export type FindSurface = 'content' | 'thread';

export type FindBarResult =
  | { status: 'idle' }
  /** Fewer than `MIN_FIND_QUERY_CHARS`: nothing is searched yet. */
  | { status: 'too-short' }
  | { status: 'found'; total: number; current: number; capped: boolean }
  /** The target did not answer: an app with no SDK, or one that is wedged. */
  | { status: 'unsearchable'; message: string };

export const findSurface = signal<FindSurface | null>(null);
export const findQuery = signal('');
export const findResult = signal<FindBarResult>({ status: 'idle' });
/** Bumped on every open that takes focus. The bar answers by focusing its field. */
export const findFocusRequest = signal(0);

const TYPING_DEBOUNCE_MS = 120;

/** Something the bar can search. `run` resolves to null when the text could
 *  not be reached, and the bar then shows `unsearchable`. `fromLine` starts
 *  the query at that line, where the target shows numbered lines. */
export interface FindTarget {
  run(query: string, step?: 1 | -1, fromLine?: number): Promise<FindResult | null>;
  clear(): void;
  unsearchable: string;
}

/** How the content pane's view is searched. Null means it has no text the bar
 *  can reach, such as an image, a PDF, the editor or a settings page. */
export type ContentFindKind = 'app' | 'preview-frame' | 'page';

export function contentFindKind(): ContentFindKind | null {
  const overlay = panelOverlay.value;
  if (overlay?.type === 'app-ui') return 'app';
  if (overlay?.type !== 'file-preview') return null;
  const repo = parseRepoPath(overlay.path);
  if (repo) {
    if (repo.mode === 'diff') return 'page';
    const body = repoPreviewBody(overlay.path, { sourceToggle: filePreviewSource.value });
    return body === 'markdown' || body === 'csv' || body === 'source' ? 'page' : null;
  }
  const body = dataPreviewBody(overlay.path, {
    sourceToggle: filePreviewSource.value,
    editing: filePreviewEditing.value,
  });
  if (body === 'html') return 'preview-frame';
  return body === 'markdown' || body === 'csv' || body === 'slides' || body === 'source' ? 'page' : null;
}

/** Whether the thread has anything to search: a real thread, not a draft. */
function transcriptSearchable(threadId: string): boolean {
  const thread = threadMap.value.get(threadId);
  return !!thread && thread.meta.state !== 'composing';
}

/** Whether `surface` shows something the bar can search. Needs no target. */
function searchable(surface: FindSurface): boolean {
  if (surface === 'content') return contentFindKind() !== null;
  const threadId = focusedThreadId.value;
  return !!threadId && transcriptSearchable(threadId);
}

/** What a query on `surface` runs against. Only an open bar asks, and the bar
 *  opens only once `findTargets` is in memory. */
function targetFor(surface: FindSurface): FindTarget | null {
  if (!searchable(surface)) return null;
  const targets = findTargets.get();
  if (surface === 'thread') return targets.threadTarget(focusedThreadId.value!);
  return targets.contentTarget(contentFindKind()!);
}

/** The surface Mod+F would search: the focused pane, if what it shows has a
 *  target. Null leaves the key to the browser's own find. */
export function focusedFindSurface(): FindSurface | null {
  const pane = isMobile() ? mobileView.value : focusedPane.value;
  if (pane !== 'content' && pane !== 'thread') return null;
  return searchable(pane) ? pane : null;
}

/** Whether `surface` shows something the bar can search, for its button. */
export function findAvailable(surface: FindSurface): boolean {
  return searchable(surface);
}

let typingTimer: ReturnType<typeof setTimeout> | null = null;
/** Each request takes a ticket, and only the newest one's answer lands. */
let latestTicket = 0;
/** The target the open session last searched, which `closeFind` clears. */
let activeTarget: FindTarget | null = null;
/** Ends the wait for the line landing a requested find runs after. */
let stopLandingWait = (): void => {};
/** The landing wait's ticket. It is still the latest while the wait pends. */
let landingTicket: number | null = null;
/** An open still waiting on `findTargets`, and the surface it is for. */
let pendingOpen: PendingOpen | null = null;
let pendingSurface: FindSurface | null = null;

function cancelTyping(): void {
  if (typingTimer !== null) clearTimeout(typingTimer);
  typingTimer = null;
}

function run(step?: 1 | -1, fromLine?: number): Promise<void> {
  cancelTyping();
  const ticket = ++latestTicket;
  const query = findQuery.value;
  const surface = findSurface.value;
  const target = surface ? targetFor(surface) : null;
  const chars = findQueryChars(query);
  if (chars < MIN_FIND_QUERY_CHARS) {
    activeTarget?.clear();
    findResult.value = chars === 0 ? { status: 'idle' } : { status: 'too-short' };
    return Promise.resolve();
  }
  if (!target) {
    findResult.value = { status: 'unsearchable', message: 'Nothing here can be searched' };
    return Promise.resolve();
  }
  activeTarget = target;
  const unsearchable: FindBarResult = { status: 'unsearchable', message: target.unsearchable };
  return Promise.resolve()
    .then(() => target.run(query, step, fromLine))
    .then(
      (count) => {
        if (ticket === latestTicket) findResult.value = count ? { status: 'found', ...count } : unsearchable;
      },
      () => {
        if (ticket === latestTicket) findResult.value = unsearchable;
      },
    );
}

/** Open the bar on `surface`. A session open elsewhere closes first: the
 *  highlight registry is one per document. Call it synchronously from the tap:
 *  the bar's field focuses a render later, outside the gesture, so this holds
 *  the keyboard for it now. Without `focus`, the caret stays where it is. */
export function openFind(surface: FindSurface, { focus = true }: { focus?: boolean } = {}): void {
  openFindThen(surface, focus, () => {});
}

/** Open the bar once `findTargets` is in memory, which is at once when warm,
 *  then run `then`. A newer open replaces a waiting one. */
function openFindThen(surface: FindSurface, focus: boolean, then: () => void): void {
  if (focus) holdSoftwareKeyboard();
  pendingOpen?.cancel();
  pendingSurface = surface;
  pendingOpen = whenLoaded(findTargets, () => {
    if (findSurface.value && findSurface.value !== surface) closeFind();
    findSurface.value = surface;
    if (focus) findFocusRequest.value++;
    then();
  }, null);
}

/** Take back an open still waiting on its code, for `surface` or for any.
 *  True when there was one. */
function takeBackOpen(surface?: FindSurface): boolean {
  if (!pendingOpen?.pending || (surface && pendingSurface !== surface)) return false;
  pendingOpen.cancel();
  return true;
}

/** `surface`'s view went away or changed what it shows. Close its session,
 *  or take back an open still waiting for it, so neither lands on the next
 *  view. */
export function endFindOn(surface: FindSurface): void {
  if (findSurface.value === surface) closeFind();
  else takeBackOpen(surface);
}

/** The bar on `surface` now shows `scope`, and its scope change has ended the
 *  previous file's session. Open the request made for it, if any. The bar
 *  takes focus, so Enter steps through the matches. A finger gets no focus,
 *  since its keyboard would cover the file. */
export function takeFindRequest(surface: FindSurface, scope: string): void {
  const request = findRequest.peek();
  if (request?.scope !== scope) return;
  findRequest.value = null;
  openFindThen(surface, !hasCoarsePointer(), () => {
    cancelTyping();
    findQuery.value = request.query;
    runAfterLineLanding(request.line);
  });
}

/** Search from `line` once the router's line landing is done. `LineNumberedCode`
 *  consumes `lineScrollTarget` as it scrolls the row into view. By then the
 *  file's text has rendered, and the find's reveal cannot fight the landing. */
function runAfterLineLanding(line: number): void {
  stopLandingWait();
  const ticket = ++latestTicket;
  landingTicket = ticket;
  const stop = effect(() => {
    if (lineScrollTarget.value !== null) return;
    // A microtask, so the effect is disposed outside its own run.
    queueMicrotask(() => {
      stop();
      if (ticket === latestTicket) void run(undefined, line);
    });
  });
  stopLandingWait = stop;
}

/** The Mod+F action. It closes an open bar, as Escape does, and otherwise
 *  opens the bar on the focused pane, when that pane has a target. */
export function toggleFocusedFind(): void {
  if (takeBackOpen()) return;
  if (findSurface.value) {
    closeFind();
    return;
  }
  const surface = focusedFindSurface();
  if (surface) openFind(surface);
}

export function toggleFind(surface: FindSurface): void {
  if (takeBackOpen(surface)) return;
  if (findSurface.value === surface) closeFind();
  else openFind(surface);
}

/** Close the bar and take the highlights out of what it searched. */
export function closeFind(): void {
  if (!findSurface.value) return;
  activeTarget?.clear();
  resetFind();
}

/** Forget the session without touching a target, for when the view goes away
 *  and takes its text with it. */
export function resetFind(): void {
  pendingOpen?.cancel();
  cancelTyping();
  stopLandingWait();
  latestTicket++;
  activeTarget = null;
  findSurface.value = null;
  findQuery.value = '';
  findResult.value = { status: 'idle' };
}

/** A keystroke in the bar. Matching waits for a pause in the typing. */
export function setFindQuery(query: string): void {
  findQuery.value = query;
  cancelTyping();
  typingTimer = setTimeout(() => void run(), TYPING_DEBOUNCE_MS);
}

/** Move to the next or previous match, wrapping at the ends. A step while a
 *  requested find waits for its line does nothing: running it would cancel
 *  the search from that line. */
export function stepFind(step: 1 | -1): Promise<void> {
  if (landingTicket === latestTicket) return Promise.resolve();
  return run(step);
}

/** The view under `surface` reloaded in place, such as an app's refresh. The
 *  old count belongs to the old document, so an open bar searches again. */
export function rerunFindOn(surface: FindSurface): Promise<void> {
  if (findSurface.value !== surface) return Promise.resolve();
  findResult.value = { status: 'idle' };
  return run();
}

/** What the bar says about the result. Empty while there is nothing to say. */
export function findStatusText(result: FindBarResult): string {
  switch (result.status) {
    case 'idle': return '';
    case 'too-short': return `Type ${MIN_FIND_QUERY_CHARS}+ characters`;
    case 'unsearchable': return result.message;
    case 'found': {
      if (result.total === 0) return 'No matches';
      const total = `${result.total}${result.capped ? '+' : ''}`;
      // Counted but not yet stepped to: the transcript waits for a step.
      if (result.current === 0) return `${total} ${result.total === 1 ? 'match' : 'matches'}`;
      return `${result.current} of ${total}`;
    }
  }
}
