import { afterEach, describe, expect, it, vi } from 'vitest';
import { drawnInkShift, inkFont, measureInkShift } from './inkCentre';

/** A canvas whose glyphs are 60 wide and paint ink over `[inkLeft, inkRight)`
 *  of their advance, wherever `fillText` puts them. */
function stubCanvas(inkLeft: number, inkRight: number) {
  const drawn: string[] = [];
  class FakeOffscreenCanvas {
    constructor(readonly width: number, readonly height: number) {}
    getContext() {
      let origin = 0;
      const canvas = this;
      return {
        font: '10px sans-serif',
        measureText: () => ({ width: 60 }),
        fillText(text: string, x: number) {
          drawn.push(`${this.font}|${text}`);
          origin = x;
        },
        getImageData: () => {
          const data = new Uint8ClampedArray(canvas.width * canvas.height * 4);
          for (let x = origin + inkLeft; x < origin + inkRight; x++) data[(10 * canvas.width + x) * 4 + 3] = 255;
          return { data };
        },
      };
    }
  }
  vi.stubGlobal('OffscreenCanvas', FakeOffscreenCanvas);
  vi.stubGlobal('document', { fonts: { check: () => true } });
  return drawn;
}

const BOLD = { fontStyle: 'normal', fontWeight: '700', fontFamily: '"Fira Code", monospace' };

describe('drawnInkShift', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('is zero for ink centred in its advance', () => {
    stubCanvas(10, 50);
    expect(drawnInkShift({ text: '8', font: '700 100px X', px: 100 })).toBe(0);
  });

  it('moves ink drawn left of its slot to the right, in em', () => {
    // Ink over 5..50 of a 60-wide advance: its middle is 2.5 left of 30.
    stubCanvas(5, 50);
    expect(drawnInkShift({ text: '3', font: '700 100px X', px: 100 })).toBeCloseTo(0.025);
  });

  it('moves ink drawn right of its slot to the left', () => {
    stubCanvas(14, 58);
    expect(drawnInkShift({ text: '1', font: '700 100px X', px: 100 })).toBeCloseTo(-0.06);
  });

  it('measures nothing it cannot draw', () => {
    stubCanvas(0, 0);
    expect(drawnInkShift({ text: ' ', font: '700 100px X', px: 100 })).toBeNull();
  });
});

describe('inkFont', () => {
  it('states weight and family at the measuring size', () => {
    expect(inkFont(BOLD)).toBe('700 100px "Fira Code", monospace');
  });

  it('keeps italic and drops what the canvas shorthand cannot parse', () => {
    expect(inkFont({ ...BOLD, fontStyle: 'italic' })).toBe('italic 700 100px "Fira Code", monospace');
    expect(inkFont({ ...BOLD, fontStyle: 'oblique 14deg' })).toBe('700 100px "Fira Code", monospace');
  });
});

describe('measureInkShift', () => {
  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('leaves the advance centred where nothing can measure', () => {
    expect(measureInkShift('3', BOLD)).toBe(0);
  });

  it('measures in the element font', () => {
    const drawn = stubCanvas(5, 50);
    expect(measureInkShift('3', BOLD)).toBeCloseTo(0.025);
    expect(drawn).toEqual(['700 100px "Fira Code", monospace|3']);
  });

  it('measures nothing for an empty badge', () => {
    const drawn = stubCanvas(5, 50);
    expect(measureInkShift('', BOLD)).toBe(0);
    expect(drawn).toEqual([]);
  });

  it('measures again until the font has loaded, then caches', () => {
    const drawn = stubCanvas(5, 50);
    let loaded = false;
    vi.stubGlobal('document', { fonts: { check: () => loaded } });
    const style = { ...BOLD, fontFamily: 'Loading' };
    measureInkShift('8', style);
    measureInkShift('8', style);
    loaded = true;
    measureInkShift('8', style);
    measureInkShift('8', style);
    expect(drawn).toHaveLength(3);
  });
});
