
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

export const findTargets = lazyModule(() => import('./find-targets'));
prefetchWhenIdle(findTargets);

export type FindSurface = 'content' | 'thread';

export type FindBarResult =
  | { status: 'idle' }
  | { status: 'too-short' }
  | { status: 'found'; total: number; current: number; capped: boolean }
  | { status: 'unsearchable'; message: string };

export const findSurface = signal<FindSurface | null>(null);
export const findQuery = signal('');
export const findResult = signal<FindBarResult>({ status: 'idle' });
export const findFocusRequest = signal(0);

const TYPING_DEBOUNCE_MS = 120;

export interface FindTarget {
  run(query: string, step?: 1 | -1, fromLine?: number): Promise<FindResult | null>;
  clear(): void;
  unsearchable: string;
}

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

function transcriptSearchable(threadId: string): boolean {
  const thread = threadMap.value.get(threadId);
  return !!thread && thread.meta.state !== 'composing';
}

function searchable(surface: FindSurface): boolean {
  if (surface === 'content') return contentFindKind() !== null;
  const threadId = focusedThreadId.value;
  return !!threadId && transcriptSearchable(threadId);
}

function targetFor(surface: FindSurface): FindTarget | null {
  if (!searchable(surface)) return null;
  const targets = findTargets.get();
  if (surface === 'thread') return targets.threadTarget(focusedThreadId.value!);
  return targets.contentTarget(contentFindKind()!);
}

export function focusedFindSurface(): FindSurface | null {
  const pane = isMobile() ? mobileView.value : focusedPane.value;
  if (pane !== 'content' && pane !== 'thread') return null;
  return searchable(pane) ? pane : null;
}

export function findAvailable(surface: FindSurface): boolean {
  return searchable(surface);
}

let typingTimer: ReturnType<typeof setTimeout> | null = null;
let latestTicket = 0;
let activeTarget: FindTarget | null = null;
let stopLandingWait = (): void => {};
let landingTicket: number | null = null;
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

export function openFind(surface: FindSurface, { focus = true }: { focus?: boolean } = {}): void {
  openFindThen(surface, focus, () => {});
}

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

function takeBackOpen(surface?: FindSurface): boolean {
  if (!pendingOpen?.pending || (surface && pendingSurface !== surface)) return false;
  pendingOpen.cancel();
  return true;
}

export function endFindOn(surface: FindSurface): void {
  if (findSurface.value === surface) closeFind();
  else takeBackOpen(surface);
}

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

function runAfterLineLanding(line: number): void {
  stopLandingWait();
  const ticket = ++latestTicket;
  landingTicket = ticket;
  const stop = effect(() => {
    if (lineScrollTarget.value !== null) return;
    queueMicrotask(() => {
      stop();
      if (ticket === latestTicket) void run(undefined, line);
    });
  });
  stopLandingWait = stop;
}

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

export function closeFind(): void {
  if (!findSurface.value) return;
  activeTarget?.clear();
  resetFind();
}

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

export function setFindQuery(query: string): void {
  findQuery.value = query;
  cancelTyping();
  typingTimer = setTimeout(() => void run(), TYPING_DEBOUNCE_MS);
}

export function stepFind(step: 1 | -1): Promise<void> {
  if (landingTicket === latestTicket) return Promise.resolve();
  return run(step);
}

export function rerunFindOn(surface: FindSurface): Promise<void> {
  if (findSurface.value !== surface) return Promise.resolve();
  findResult.value = { status: 'idle' };
  return run();
}

export function findStatusText(result: FindBarResult): string {
  switch (result.status) {
    case 'idle': return '';
    case 'too-short': return `Type ${MIN_FIND_QUERY_CHARS}+ characters`;
    case 'unsearchable': return result.message;
    case 'found': {
      if (result.total === 0) return 'No matches';
      const total = `${result.total}${result.capped ? '+' : ''}`;
      if (result.current === 0) return `${total} ${result.total === 1 ? 'match' : 'matches'}`;
      return `${result.current} of ${total}`;
    }
  }
}
