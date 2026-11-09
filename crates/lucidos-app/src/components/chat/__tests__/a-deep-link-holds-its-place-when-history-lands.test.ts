/** A notification deep link lands on its event and then STAYS there.
 *
 *  The reported sequence: tap the notification for a question, land on the
 *  question card, read for a few seconds. The transcript then carries you up
 *  into history you never asked for.
 *
 *  What moves it is history folding in at the FRONT while the link resolves.
 *  A target older than the newest page makes the link page history toward it.
 *  A read already running when the view mounts folds in late too. WebKit
 *  implements no scroll anchoring, so the container keeps its offset while the
 *  content under it slides down.
 *
 *  So each read takes the same hold a scroll-driven backfill takes. Two things
 *  differ. The hold names no turn to re-point the window onto, since the link
 *  claims its own. And it names the turn the READER is on: the paging runs for
 *  seconds, and a height delta cannot tell growth above them from below. */
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

/** An exchange as `exchangeKey` reads one: its boundary event's id. */
function ex(id: string): Exchange {
  return { userEvent: { _eventId: id }, userSeq: 1, steps: [] } as unknown as Exchange;
}

/** One turn in the stub below, at a known position in the viewport. */
type Turn = { id: string; top: number };

/** A transcript stub carrying turns at known positions. Enough DOM for
 *  `readScrollAnchor` and `anchorTargetTop`, which is all a hold reads. */
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

/** The reader, parked 40px into the turn the link landed them on. */
const landedOn: Turn[] = [{ id: 'older', top: -900 }, { id: 'landed-on', top: -40 }];
const landed = () => transcript({ scrollTop: 4200, scrollHeight: 9000, turns: landedOn });

/** The same transcript once 6,000px of older history has folded in above. */
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
    // The re-point is what a scroll-driven backfill needs and this one must not
    // have: it grows the window a step per fold, drawing turns nobody asked for
    // on every page the link reads toward its target.
    const after = [ex('old-1'), ex('old-2'), ex('landed-on')];
    expect(anchorAfterBackfill(after, readerOnlyHold(landed()))).toBeNull();
  });

  it('puts the reader back on their turn, wherever the fold moved it', () => {
    expect(holdTargetTop(afterFold(15000), readerOnlyHold(landed()))).toBe(10200);
  });

  it('ignores a reply drawn BELOW the reader while the fetch ran', () => {
    // The height delta cannot: it reads 500px of streamed reply as more history
    // above, and shoves the reader that far past their own turn. This is why
    // the hold names a turn at all.
    expect(holdTargetTop(afterFold(15500), readerOnlyHold(landed()))).toBe(10200);
  });

  it('falls back to the height delta for a hold naming no turn', () => {
    // A scroll-driven backfill's hold, whose read is one short round trip.
    const hold = { ...readerOnlyHold(landed()), anchor: null };
    expect(holdTargetTop(afterFold(15000, []), hold)).toBe(10200);
  });

  it('falls back again when the fold absorbed the turn the hold named', () => {
    const gone: Turn[] = [{ id: 'merged-into-another-turn', top: 5960 }];
    expect(holdTargetTop(afterFold(15000, gone), readerOnlyHold(landed()))).toBe(10200);
  });
});

/** The DEPENDENCY ARRAY of the effect that pages history toward a deep
 *  link's target. It is that effect's identity, so the body below is sliced
 *  back from it rather than forward from a guard the effect shares with others.
 *
 *  `historyReadSettled` is in it because each settle asks for the next page,
 *  and because the ask stands down while another read is running. */
const HISTORY_EFFECT_DEPS =
  '}, [threadId, linkTarget, hasOlderEvents, historyReadSettled]);';

/** That effect's whole body. A scan over the file cannot name one effect, and
 *  the chevron reaches for history too, on purpose. */
function historyEffectBody(): string {
  const to = SOURCE.indexOf(HISTORY_EFFECT_DEPS);
  expect(to, HISTORY_EFFECT_DEPS).toBeGreaterThan(-1);
  const from = SOURCE.lastIndexOf('useEffect(() => {', to);
  expect(from, 'the effect opens above its dependency array').toBeGreaterThan(-1);
  return SOURCE.slice(from, to);
}

/** The body of `requestLinkPage`, the deep link's one page read. */
function linkPageBody(): string {
  const from = SOURCE.indexOf('function requestLinkPage(');
  expect(from, 'requestLinkPage is defined').toBeGreaterThan(-1);
  return SOURCE.slice(from, SOURCE.indexOf('\n}\n', from));
}

describe('ThreadView pages history toward a deep link through the hold', () => {
  it('fetches the whole history only for the up chevron', () => {
    // The chevron is taking the reader to the top on purpose. A deep link
    // reads page by page and stops at its target's page. That keeps a long
    // thread from being fetched and drawn whole on a phone.
    // `showEventWhereItLives` is a caller elsewhere, and it reads before the
    // thread is focused, so no hold of this thread's can be waiting on it.
    expect(SOURCE.match(/ensureWholeThreadLoaded\(/g)).toHaveLength(1);
  });

  it('asks for a page only while no loaded turn holds the target', () => {
    // A target on the newest page fetches nothing more.
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
    // Standing down is what keeps the hold honest, since a read queued behind
    // another folds with no hold of its own. Holding without asking is also
    // the whole of a reader who left and came back inside the fetch.
    const body = historyEffectBody();
    expect(body).toContain('if (threadHistoryReadInFlight(threadId)) {');
    expect(body).toContain('historyHoldByThread.set(threadId, readerOnlyHold(el));');
  });

  it('settles the chevron read too, so a hold taken for it is consumed', () => {
    // The press is not the only way into a hold on that read: a reader
    // returning mid-fetch takes one for whatever is running.
    const from = SOURCE.indexOf('void ensureWholeThreadLoaded(threadId, ');
    expect(from, 'the chevron reaches for the whole history').toBeGreaterThan(-1);
    const call = SOURCE.slice(from, from + 400);
    expect(call).toContain('settleHistoryRead(threadId, added, onHistoryRead)');
    // And it lands only once the reader's scroll hold ends (ADR 0258).
    expect(call).toContain('scrollHoldReleased(el)');
  });

  it('stops paging on a page that adds nothing, and clears that on the way out', () => {
    // A failed fetch leaves `hasOlderEvents` true and toasts, so an unbounded
    // retry is a loop the reader watches. The link's deadline then reports it.
    const body = linkPageBody();
    expect(body).toContain('if (linkPagingGaveUp.has(threadId)) return;');
    expect(body).toContain('if (!added) linkPagingGaveUp.add(threadId);');
    expect(SOURCE).toContain('linkPagingGaveUp.delete(threadId);');
  });
});
