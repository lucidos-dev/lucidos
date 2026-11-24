
import { createFinder, createPainter, highlightApiPresent, type ClearBand, type FindResult } from '@lucidos/find';
import { askFrame, tellFrame } from './app-bridge';
import { getVisibleAppFrame } from './apps';
import type { ContentFindKind, FindTarget } from './find-bar';
import { transcriptTarget } from './transcript-find';
import { isElementVisible } from '../../utils/elementVisibility';
import { PREVIEW_FRAME_ROLE, askPreviewFind, postToPreviewFrame } from '../../utils/previewFrameProtocol';

function isCount(n: unknown): n is number {
  return typeof n === 'number' && Number.isInteger(n) && n >= 0;
}

export function parseFindReply(value: unknown): FindResult | null {
  const v = value as { total?: unknown; current?: unknown; capped?: unknown } | null;
  if (!v || typeof v !== 'object') return null;
  const { total, current } = v;
  if (!isCount(total) || !isCount(current) || current > total) return null;
  if ((total === 0) !== (current === 0)) return null;
  return { total, current, capped: v.capped === true };
}

export const FIND_TIMEOUT_MS = 2000;

function visibleElement(selector: string): HTMLElement | null {
  for (const el of document.querySelectorAll<HTMLElement>(selector)) {
    if (isElementVisible(el)) return el;
  }
  return null;
}

const FILE_UNSEARCHABLE = 'This file can’t be searched';

const hostPainter = createPainter(highlightApiPresent);

function pageRoot(): Element | null {
  const body = visibleElement('.file-preview-frame-body');
  return body?.querySelector('.repo-preview-split-main') ?? body;
}

function pageBand(): ClearBand | undefined {
  const bar = visibleElement('.file-preview-frame > .find-bar-slot');
  return bar ? { top: bar.getBoundingClientRect().bottom, bottom: window.innerHeight } : undefined;
}

const pageFinder = createFinder(pageRoot, hostPainter, pageBand);

function lineRow(line: number | undefined): Element | undefined {
  if (line === undefined) return undefined;
  return pageRoot()?.querySelector(`[data-line="${line}"]`) ?? undefined;
}

const pageTarget: FindTarget = {
  run: (query, step, fromLine) => Promise.resolve(pageFinder.find(query, step, lineRow(fromLine))),
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

const CONTENT_TARGETS: Record<ContentFindKind, FindTarget> = {
  app: appTarget,
  'preview-frame': previewFrameTarget,
  page: pageTarget,
};

export function contentTarget(kind: ContentFindKind): FindTarget {
  return CONTENT_TARGETS[kind];
}

export function threadTarget(threadId: string): FindTarget {
  return transcriptTarget(threadId, hostPainter);
}
