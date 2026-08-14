import { describe, it, expect, afterEach } from 'vitest';
import { computeFitsInOneRow, contentWidthOf, countGappedPairs } from './useFitsInOneRow';

// Pinning the actual width math the prompt uses to decide whether to lift the
// secondary button (the Diff button — in the banner or standalone while
// composing) to a row above. Each scenario walks a real-ish set of button
// widths the bottom row of PromptInput would carry on a phone screen.
describe('computeFitsInOneRow', () => {
  // 0.5rem at the default 16px root = 8px between items.
  const gap = 8;

  it('an empty row trivially fits', () => {
    expect(computeFitsInOneRow([], 320, gap)).toBe(true);
  });

  it('a single item fits when it is narrower than the container', () => {
    expect(computeFitsInOneRow([100], 320, gap)).toBe(true);
  });

  it('a single item that is wider than the container does not fit', () => {
    expect(computeFitsInOneRow([400], 320, gap)).toBe(false);
  });

  // [icons 36+36] [Save 70] [Diff 56] [Discard 84] [Apply 64] = 346px content
  // + 5 gaps of 8px = 386px total. A 393pt iPhone Pro at default zoom has
  // ~330px of usable inside the prompt-actions-area (after 0.5rem + 0.75rem
  // padding) — does NOT fit, must lift.
  it('the full banner row does not fit a phone-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 330, gap)).toBe(false);
  });

  // Same items in a desktop-width container clearly fit.
  it('the full banner row fits a desktop-width container', () => {
    expect(computeFitsInOneRow([36, 36, 70, 56, 84, 64], 800, gap)).toBe(true);
  });

  // [icons 36+36] [secondary 110] [Save 70] [Send 60] = 312px content
  // + 4 gaps of 8px = 344px. Still over 330px on a phone — must lift the
  // secondary button.
  it('a dense bottom row (icons + a wide secondary + Save + Send) overflows a phone', () => {
    expect(computeFitsInOneRow([36, 36, 110, 70, 60], 330, gap)).toBe(false);
  });

  // After lifting the secondary button, the bottom row is just icons + Save +
  // Send. The hook still measures every [data-row-item] (the lifted button is a
  // data-row-item too, just in a sibling row), so the sum is unchanged —
  // stays "does not fit" and stays stacked. Loop avoided.
  it('still reports "does not fit" with the same item set after lifting (stable across re-render)', () => {
    const widths = [36, 36, 110, 70, 60];
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
    // Same widths, container hasn't grown — same answer.
    expect(computeFitsInOneRow(widths, 330, gap)).toBe(false);
  });

  // Sub-pixel rounding tolerance: a sum that is 0.4px over still counts as
  // fitting (browsers round and a 0.5px epsilon prevents flicker).
  it('tolerates sub-pixel rounding within 0.5px', () => {
    expect(computeFitsInOneRow([100.4], 100, 0)).toBe(true);
    expect(computeFitsInOneRow([100.6], 100, 0)).toBe(false);
  });

  // Larger root font size (user accessibility setting) → larger gap → row
  // overflows where it would have fit at 16px. Caller passes the scaled
  // gap in, so the math just sees a larger gap.
  it('honors a larger gap when the user has scaled their font size up', () => {
    expect(computeFitsInOneRow([100, 100, 100], 320, 8)).toBe(true);
    expect(computeFitsInOneRow([100, 100, 100], 320, 16)).toBe(false);
  });

  // The composer's row declares no `gap` of its own, so only the right-hand
  // cluster's pairs cost anything. Charging one per adjacency billed four gaps
  // the row never spends, which is what lifted Diff off a row holding it.
  it('charges only the gaps the caller says exist', () => {
    const widths = [40, 40, 40, 63, 98];
    // One real gap: fits. Four phantom ones on top: does not.
    expect(computeFitsInOneRow(widths, 300, 9, 1)).toBe(true);
    expect(computeFitsInOneRow(widths, 300, 9)).toBe(false);
  });

  it('charges nothing for a lone item, whatever the gap', () => {
    expect(computeFitsInOneRow([100], 100, 9, 0)).toBe(true);
  });
});

interface FakeEl {
  klass: string;
  item: boolean;
  kids: FakeEl[];
  querySelectorAll(selector: string): FakeEl[];
}

/** A stand-in element answering DESCENDANT queries, which is the only thing
 *  `countGappedPairs` asks of the DOM. The project runs Vitest without jsdom,
 *  so a real row cannot be built here. Nesting is what tells a cluster-wide
 *  count apart from a per-parent one, so the stand-in has to model it. */
function el(klass: string, kids: FakeEl[] = [], item = false): FakeEl {
  const node: FakeEl = {
    klass,
    item,
    kids,
    querySelectorAll(selector) {
      const all: FakeEl[] = [];
      const walk = (n: FakeEl) => n.kids.forEach((k) => { all.push(k); walk(k); });
      walk(node);
      const matches = selector === '[data-row-item]'
        ? (n: FakeEl) => n.item
        : (n: FakeEl) => `.${n.klass}` === selector;
      return all.filter(matches);
    },
  };
  return node;
}

const item = () => el('', [], true);

/** A `.prompt-actions-row`: three ungapped leading icons, then the gapped
 *  `.prompt-actions-right` cluster. `stacked` splits that cluster across two
 *  sub-rows, exactly as `.is-stacked` does. */
function promptRow(stacked: boolean): HTMLElement {
  const right = el('prompt-actions-right', stacked
    ? [el('prompt-actions-subrow', [item()]), el('prompt-actions-subrow', [item()])]
    : [item(), item()]);
  return el('prompt-actions-row', [item(), item(), item(), right]) as unknown as HTMLElement;
}

describe('countGappedPairs', () => {
  it('charges every adjacency when no cluster is named', () => {
    expect(countGappedPairs(promptRow(false))).toBe(4);
  });

  it('charges only the named cluster', () => {
    expect(countGappedPairs(promptRow(false), '.prompt-actions-right')).toBe(1);
  });

  // The point of reading through the cluster rather than each item's parent. A
  // per-parent count drops to 0 here, so it reports a narrower row than the
  // unstacked one needs. That unstacks the row and stacks it again next
  // measurement.
  it('gives the same count stacked and unstacked', () => {
    const flat = countGappedPairs(promptRow(false), '.prompt-actions-right');
    const split = countGappedPairs(promptRow(true), '.prompt-actions-right');
    expect(flat).toBe(1);
    expect(split).toBe(flat);
  });

  it('charges nothing when the cluster is absent', () => {
    const bare = el('prompt-actions-row', [item(), item()]) as unknown as HTMLElement;
    expect(countGappedPairs(bare, '.prompt-actions-right')).toBe(0);
  });
});

describe('contentWidthOf', () => {
  const realGetComputedStyle = globalThis.getComputedStyle;
  afterEach(() => { globalThis.getComputedStyle = realGetComputedStyle; });

  /** Nothing here computes layout, so `clientWidth` and the resolved padding
   *  are both supplied. The row's real padding is `0 0.75rem 0.5rem 0.5rem`. */
  function padded(clientWidth: number, left: string, right: string): HTMLElement {
    globalThis.getComputedStyle = (() => (
      { paddingLeft: left, paddingRight: right }
    )) as unknown as typeof getComputedStyle;
    return { clientWidth } as HTMLElement;
  }

  it('subtracts the container\'s own horizontal padding', () => {
    expect(contentWidthOf(padded(347, '8px', '12px'))).toBe(327);
  });

  it('returns clientWidth when the container has no padding', () => {
    expect(contentWidthOf(padded(347, '0px', '0px'))).toBe(347);
  });

  it('reads an unresolvable padding as none rather than NaN', () => {
    expect(contentWidthOf(padded(347, '', ''))).toBe(347);
  });

  it('never goes negative', () => {
    expect(contentWidthOf(padded(10, '40px', '0px'))).toBe(0);
  });
});
