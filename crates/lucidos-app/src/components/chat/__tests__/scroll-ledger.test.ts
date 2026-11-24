import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: same
import { join, relative } from 'node:path';

import {
  type NavScrollKind,
  forgetLeftPosition,
  holdPosition,
  holdsReaderOn,
  isWhereWeHeldIt,
  isWhereWeLastScrolledIt,
  lastWriteKind,
  onRebasedScroll,
  writeLedgerScroll,
  wroteWithinEventWindow,
} from '../scrollLedger';
import { srcPath } from '../../../__tests__/staticImportGraph';

describe('the write ledger', () => {
  const container = (scrollTop = 0) => ({ scrollTop }) as HTMLElement;

  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => {
    holdPosition(null);
    vi.useRealTimers();
  });

  it('knows nothing about a container the app never wrote', () => {
    const el = container(40);
    expect(lastWriteKind(el)).toBeNull();
    expect(wroteWithinEventWindow(el)).toBe(false);
    expect(isWhereWeLastScrolledIt(el)).toBe(false);
  });

  it('records where the browser settled the write, not what was asked', () => {
    const el = { get scrollTop() { return 90; }, set scrollTop(_top: number) {} } as HTMLElement;
    writeLedgerScroll(el, 500, 'placement');
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('keeps one record per container, so a write to one never answers for another', () => {
    const transcript = container();
    const pane = container();
    writeLedgerScroll(transcript, 100, 'held');
    writeLedgerScroll(pane, 300, 'placement');
    expect(lastWriteKind(transcript)).toBe('held');
    expect(wroteWithinEventWindow(transcript)).toBe(true);
    expect(isWhereWeLastScrolledIt(transcript)).toBe(true);
    expect(lastWriteKind()).toBe('placement');
  });

  it('closes the event window by the clock and keeps the position past it', () => {
    const el = container();
    writeLedgerScroll(el, 100, 'placement');
    vi.advanceTimersByTime(1000);
    expect(wroteWithinEventWindow(el)).toBe(false);
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
    el.scrollTop = 101;
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
  });

  it('retires a position the container left, so a return onto it is the reader', () => {
    const el = container();
    writeLedgerScroll(el, 100, 'placement');
    forgetLeftPosition(el);
    expect(isWhereWeLastScrolledIt(el)).toBe(true);
    el.scrollTop = 400;
    forgetLeftPosition(el);
    el.scrollTop = 100;
    expect(isWhereWeLastScrolledIt(el)).toBe(false);
    expect(lastWriteKind(el)).toBe('placement');
  });

  it('records a carry that shares another write\'s event as held', () => {
    const el = container();
    const told: NavScrollKind[] = [];
    const unsubscribe = onRebasedScroll((_el, kind) => told.push(kind));
    writeLedgerScroll(el, 100, 'held');
    writeLedgerScroll(el, 110, 'carry');
    expect(lastWriteKind(el)).toBe('held');
    vi.advanceTimersByTime(1000);
    writeLedgerScroll(el, 120, 'carry');
    writeLedgerScroll(el, 130, 'carry');
    unsubscribe();
    expect(lastWriteKind(el)).toBe('carry');
    expect(told).toEqual(['held', 'held', 'carry', 'carry']);
  });

  it('judges a carry by its own container, not by a write to another one', () => {
    const transcript = container();
    const pane = container();
    writeLedgerScroll(transcript, 100, 'held');
    vi.advanceTimersByTime(1000);
    writeLedgerScroll(pane, 300, 'placement');
    writeLedgerScroll(transcript, 110, 'carry');
    expect(lastWriteKind(transcript)).toBe('carry');
  });

  it('holds one container at a time, by position', () => {
    const a = container(50);
    const b = container(70);
    holdPosition(a);
    expect(isWhereWeHeldIt(a)).toBe(true);
    a.scrollTop = 51;
    expect(isWhereWeHeldIt(a)).toBe(true);
    a.scrollTop = 60;
    expect(isWhereWeHeldIt(a)).toBe(false);
    expect(holdsReaderOn(a)).toBe(true);
    holdPosition(b);
    expect(holdsReaderOn(a)).toBe(false);
    expect(isWhereWeHeldIt(b)).toBe(true);
    holdPosition(null);
    expect(holdsReaderOn(b)).toBe(false);
  });
});

describe('scrollTop has one writer', () => {
  const DIRECT_WRITERS: Record<string, string> = {
    'components/chat/scrollLedger.ts': 'the one writer',
    'utils/webkitRepaint.ts': 'the repaint nudge, a deliberate ±1 the ledger tolerates',
    'components/shared/overlayExitDrawing.ts': 'a detached clone of a closing overlay',
    'components/drawer/LeavingViewDrawing.tsx': 'a detached clone of a leaving view',
    'components/drawer/WindowedRows.tsx': 'the drawer list, whose reveal jump the ledger does not record',
    'components/chat/promptResize.ts': 'the prompt textarea',
    'components/chat/promptValueSync.ts': 'the prompt textarea',
    'components/chat/RungPicker.tsx': 'the picker panel',
    'hooks/useAnchoredPopover.ts': 'a popover panel',
    'utils/shellScrollGuard.ts': 'the shell, which must never scroll',
  };

  function sourceFiles(dir: string): string[] {
    return (readdirSync(dir) as string[]).flatMap((name) => {
      const path = join(dir, name);
      if (statSync(path).isDirectory()) return name === '__tests__' || name === 'generated' ? [] : sourceFiles(path);
      return /\.tsx?$/.test(name) && !/\.test\.tsx?$/.test(name) ? [path] : [];
    });
  }

  it('assigns scrollTop only in the files that say why', () => {
    const root = srcPath('');
    const writers = sourceFiles(root)
      .filter((path) => /\.scrollTop\s*[-+]?=(?!=)/.test(readFileSync(path, 'utf8')))
      .map((path) => (relative(root, path) as string).split('\\').join('/'))
      .sort();
    expect(writers).toEqual(Object.keys(DIRECT_WRITERS).sort());
  });
});
