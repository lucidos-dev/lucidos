/**
 * The find bar: one bar, many targets.
 *
 * A find session belongs to one surface, the content pane or the thread pane,
 * and searches what that surface shows. A target answers a query however it
 * reaches its text. An app frame is isolated (ADR 0227), so it is asked over
 * the app bridge, and its SDK matches there. The HTML preview is isolated too,
 * and its bridge carries a copy of the same matcher. A text preview is the
 * host's own DOM, so the matcher runs here (`@lucidos/find`). The transcript
 * counts the thread's data and draws a match on a step (`transcript-find.ts`).
 *
 * Plans: `docs/plans/2026-10-07-find-in-app.md` and
 * `docs/plans/2026-10-07-find-in-previews-and-transcript.md`.
 */

import { signal } from '@preact/signals';
import { createFinder, createPainter, highlightApiPresent, type FindResult } from '@lucidos/find';
import { askFrame, tellFrame } from './app-bridge';
import { getVisibleAppFrame } from './apps';
import {
  filePreviewEditing, filePreviewSource, focusedPane, focusedThreadId, mobileView, panelOverlay,
} from '../store';
import { transcriptSearchable, transcriptTarget } from './transcript-find';
import { parseRepoPath } from '../repoPath';
import { dataPreviewBody, repoPreviewBody } from '../../components/files/previewBody';
import { isElementVisible } from '../../components/chat/scrollState';
import { isMobile } from '../../utils/viewport';
import { PREVIEW_FRAME_ROLE, askPreviewFind, postToPreviewFrame } from '../../utils/previewFrameProtocol';

/** Where a find bar lives, and what it searches. */
export type FindSurface = 'content' | 'thread';

export type FindBarResult =
  | { status: 'idle' }
  | { status: 'found'; total: number; current: number; capped: boolean }
  /** The target did not answer: an app with no SDK, or one that is wedged. */
  | { status: 'unsearchable'; message: string };

export const findSurface = signal<FindSurface | null>(null);
export const findQuery = signal('');
export const findResult = signal<FindBarResult>({ status: 'idle' });
/** Bumped on every open, so Mod+F on an open bar puts the caret back in it. */
export const findFocusRequest = signal(0);

/** How long an app frame gets to answer. An app with no SDK never does. */
export const FIND_TIMEOUT_MS = 2000;
const TYPING_DEBOUNCE_MS = 120;

/** Something the bar can search. `run` resolves to null when the text could
 *  not be reached, and the bar then shows `unsearchable`. */
export interface FindTarget {
  run(query: string, step?: 1 | -1): Promise<FindResult | null>;
  clear(): void;
  unsearchable: string;
}

function isCount(n: unknown): n is number {
  return typeof n === 'number' && Number.isInteger(n) && n >= 0;
}

/** Read a frame's answer. It crossed a `postMessage` from code the host does
 *  not trust, so anything but a well-formed count is refused. */
export function parseFindReply(value: unknown): FindResult | null {
  const v = value as { total?: unknown; current?: unknown; capped?: unknown } | null;
  if (!v || typeof v !== 'object') return null;
  const { total, current } = v;
  if (!isCount(total) || !isCount(current) || current > total) return null;
  if ((total === 0) !== (current === 0)) return null;
  return { total, current, capped: v.capped === true };
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

function visibleElement(selector: string): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(selector)) {
    if (isElementVisible(el)) return el;
  }
  return null;
}

const FILE_UNSEARCHABLE = 'This file can’t be searched';

/** The highlight registry is the document's, so the host has one painter. */
const hostPainter = createPainter(highlightApiPresent);

/** A text preview's body, without the repo preview's file tree beside it. */
function pageRoot(): Element | null {
  const body = visibleElement('.file-preview-frame-body');
  return body?.querySelector('.repo-preview-split-main') ?? body;
}

const pageFinder = createFinder(pageRoot, hostPainter);

const pageTarget: FindTarget = {
  run: (query, step) => Promise.resolve(pageFinder.find(query, step)),
  clear: () => pageFinder.clear(),
  unsearchable: FILE_UNSEARCHABLE,
};

const appTarget: FindTarget = {
  run(query, step) {
    const frame = getVisibleAppFrame();
    if (!frame) return Promise.resolve(null);
    return askFrame(frame, 'find', step ? { query, step } : { query }, FIND_TIMEOUT_MS)
      .then(parseFindReply, () => null);
  },
  clear() {
    const frame = getVisibleAppFrame();
    if (frame) tellFrame(frame, 'find', { clear: true });
  },
  unsearchable: 'This app can’t be searched',
};

function previewFrameWindow(): Window | null {
  const frame = visibleElement(`iframe[data-role="${PREVIEW_FRAME_ROLE}"]`) as HTMLIFrameElement | null;
  return frame?.contentWindow ?? null;
}

const previewFrameTarget: FindTarget = {
  run(query, step) {
    const win = previewFrameWindow();
    if (!win) return Promise.resolve(null);
    return askPreviewFind(win, step ? { query, step } : { query }, FIND_TIMEOUT_MS)
      .then(parseFindReply, () => null);
  },
  clear() {
    postToPreviewFrame(previewFrameWindow(), { kind: 'find', id: '', args: { clear: true } });
  },
  unsearchable: FILE_UNSEARCHABLE,
};

function targetFor(surface: FindSurface): FindTarget | null {
  if (surface === 'thread') {
    const threadId = focusedThreadId.value;
    return threadId && transcriptSearchable(threadId) ? transcriptTarget(threadId, hostPainter) : null;
  }
  switch (contentFindKind()) {
    case 'app': return appTarget;
    case 'preview-frame': return previewFrameTarget;
    case 'page': return pageTarget;
    default: return null;
  }
}

/** The surface Mod+F would search: the focused pane, if what it shows has a
 *  target. Null leaves the key to the browser's own find. */
export function focusedFindSurface(): FindSurface | null {
  const pane = isMobile() ? mobileView.value : focusedPane.value;
  if (pane !== 'content' && pane !== 'thread') return null;
  return targetFor(pane) ? pane : null;
}

/** Whether `surface` shows something the bar can search, for its button. */
export function findAvailable(surface: FindSurface): boolean {
  return targetFor(surface) !== null;
}

let typingTimer: ReturnType<typeof setTimeout> | null = null;
/** Each request takes a ticket, and only the newest one's answer lands. */
let latestTicket = 0;
/** The target the open session last searched, which `closeFind` clears. */
let activeTarget: FindTarget | null = null;

function cancelTyping(): void {
  if (typingTimer !== null) clearTimeout(typingTimer);
  typingTimer = null;
}

function run(step?: 1 | -1): Promise<void> {
  cancelTyping();
  const ticket = ++latestTicket;
  const query = findQuery.value;
  const surface = findSurface.value;
  const target = surface ? targetFor(surface) : null;
  if (!query.trim()) {
    activeTarget?.clear();
    findResult.value = { status: 'idle' };
    return Promise.resolve();
  }
  if (!target) {
    findResult.value = { status: 'unsearchable', message: 'Nothing here can be searched' };
    return Promise.resolve();
  }
  activeTarget = target;
  const unsearchable: FindBarResult = { status: 'unsearchable', message: target.unsearchable };
  return Promise.resolve()
    .then(() => target.run(query, step))
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
 *  highlight registry is one per document. */
export function openFind(surface: FindSurface): void {
  if (findSurface.value && findSurface.value !== surface) closeFind();
  findSurface.value = surface;
  findFocusRequest.value++;
}

/** The Mod+F action: open the bar on the focused pane, when it has a target. */
export function openFocusedFind(): void {
  const surface = focusedFindSurface();
  if (surface) openFind(surface);
}

export function toggleFind(surface: FindSurface): void {
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
  cancelTyping();
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

/** Move to the next or previous match, wrapping at the ends. */
export function stepFind(step: 1 | -1): Promise<void> {
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
