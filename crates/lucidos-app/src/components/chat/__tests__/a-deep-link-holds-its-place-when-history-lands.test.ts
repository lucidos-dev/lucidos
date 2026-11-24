import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { anchorAfterBackfill, holdTargetTop, readerOnlyHold } from '../ThreadView';
import type { Exchange } from '../../../store/thread-events';

const here: string = dirname(fileURLToPath(import.meta.url));
const SOURCE: string = readFileSync(resolve(here, '../ThreadView.tsx'), 'utf8');

function ex(id: string): Exchange {
  return { userEvent: { _eventId: id }, userSeq: 1, steps: [] } as unknown as Exchange;
}

type Turn = { id: string; top: number };

function transcript(o: { scrollTop: number; scrollHeight: number; turns: Turn[] }): HTMLElement {
  const kids = o.turns.map(t => ({
    getBoundingClientRect: () => ({ top: t.top, height: 120 }),
    getAttribute: (name: string) => (name === 'data-event-id' ? t.id : null),
  }));
  return {
    scrollTop: o.scrollTop,
    scrollHeight: o.scrollHeight,
    children: kids,
    getBoundingClientRect: () => ({ top: 0 }),
    querySelector: (sel: string) => {
      if (sel === '.thread-feed') return null;
      const wanted = /data-event-id="(.*)"/.exec(sel)?.[1];
      return kids[o.turns.findIndex(t => t.id === wanted)] ?? null;
    },
  } as unknown as HTMLElement;
}

const landedOn: Turn[] = [{ id: 'older', top: -900 }, { id: 'landed-on', top: -40 }];
const landed = () => transcript({ scrollTop: 4200, scrollHeight: 9000, turns: landedOn });

const afterFold = (scrollHeight: number, turns?: Turn[]) => transcript({
  scrollTop: 4200,
  scrollHeight,
  turns: turns ?? [{ id: 'older', top: 5100 }, { id: 'landed-on', top: 5960 }],
});

describe("a deep link's history read holds the reader", () => {
  it('captures the frame and the turn the reader is parked on', () => {
    expect(readerOnlyHold(landed())).toMatchObject({
      prevScrollTop: 4200,
      prevScrollHeight: 9000,
      anchor: { eventId: 'landed-on', relTop: -40 },
    });
  });

  it('names no turn to re-point, so the window is left alone', () => {
    const after = [ex('old-1'), ex('old-2'), ex('landed-on')];
    expect(anchorAfterBackfill(after, readerOnlyHold(landed()))).toBeNull();
  });

  it('puts the reader back on their turn, wherever the fold moved it', () => {
    expect(holdTargetTop(afterFold(15000), readerOnlyHold(landed()))).toBe(10200);
  });

  it('ignores a reply drawn BELOW the reader while the fetch ran', () => {
    expect(holdTargetTop(afterFold(15500), readerOnlyHold(landed()))).toBe(10200);
  });

  it('falls back to the height delta for a hold naming no turn', () => {
    const hold = { ...readerOnlyHold(landed()), anchor: null };
    expect(holdTargetTop(afterFold(15000, []), hold)).toBe(10200);
  });

  it('falls back again when the fold absorbed the turn the hold named', () => {
    const gone: Turn[] = [{ id: 'merged-into-another-turn', top: 5960 }];
    expect(holdTargetTop(afterFold(15000, gone), readerOnlyHold(landed()))).toBe(10200);
  });
});

const HISTORY_EFFECT_DEPS =
  '}, [threadId, linkTarget, hasOlderEvents, historyReadSettled]);';

function historyEffectBody(): string {
  const to = SOURCE.indexOf(HISTORY_EFFECT_DEPS);
  expect(to, HISTORY_EFFECT_DEPS).toBeGreaterThan(-1);
  const from = SOURCE.lastIndexOf('useEffect(() => {', to);
  expect(from, 'the effect opens above its dependency array').toBeGreaterThan(-1);
  return SOURCE.slice(from, to);
}

function linkPageBody(): string {
  const from = SOURCE.indexOf('function requestLinkPage(');
  expect(from, 'requestLinkPage is defined').toBeGreaterThan(-1);
  return SOURCE.slice(from, SOURCE.indexOf('\n}\n', from));
}

describe('ThreadView pages history toward a deep link through the hold', () => {
  it('fetches the whole history only for the up chevron', () => {
    expect(SOURCE.match(/ensureWholeThreadLoaded\(/g)).toHaveLength(1);
  });

  it('asks for a page only while no loaded turn holds the target', () => {
    const body = historyEffectBody();
    expect(body).toContain('deepLinkTargetIndex(exchangesRef.current, linkTarget) >= 0) return;');
    expect(body).toContain('requestLinkPage(');
    expect(body).not.toContain('ensureWholeThreadLoaded(');
  });

  it('reads ONE page, held for the reader alone', () => {
    const body = linkPageBody();
    expect(body).toContain('historyHoldByThread.set(threadId, readerOnlyHold(el));');
    expect(body).toContain('loadOlderThreadEvents(threadId, ');
    expect(body).toContain('scrollHoldReleased(el)');
  });

  it('holds the reader for a read already running, and asks for nothing', () => {
    const body = historyEffectBody();
    expect(body).toContain('if (threadHistoryReadInFlight(threadId)) {');
    expect(body).toContain('historyHoldByThread.set(threadId, readerOnlyHold(el));');
  });

  it('settles the chevron read too, so a hold taken for it is consumed', () => {
    const from = SOURCE.indexOf('void ensureWholeThreadLoaded(threadId, ');
    expect(from, 'the chevron reaches for the whole history').toBeGreaterThan(-1);
    const call = SOURCE.slice(from, from + 400);
    expect(call).toContain('settleHistoryRead(threadId, added, onHistoryRead)');
    expect(call).toContain('scrollHoldReleased(el)');
  });

  it('stops paging on a page that adds nothing, and clears that on the way out', () => {
    const body = linkPageBody();
    expect(body).toContain('if (linkPagingGaveUp.has(threadId)) return;');
    expect(body).toContain('if (!added) linkPagingGaveUp.add(threadId);');
    expect(SOURCE).toContain('linkPagingGaveUp.delete(threadId);');
  });
});
