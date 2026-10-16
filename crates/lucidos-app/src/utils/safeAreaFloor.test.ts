import { afterEach, describe, expect, it, vi } from 'vitest';
import {
  createSafeAreaMemory,
  foldSafeAreaReading,
  viewportShapeKey,
  type SafeAreaInsets,
} from './safeAreaFloor';

const PORTRAIT: SafeAreaInsets = { top: 59, right: 0, bottom: 34, left: 0 };
const LANDSCAPE: SafeAreaInsets = { top: 0, right: 59, bottom: 21, left: 59 };
const LOST: SafeAreaInsets = { top: 0, right: 0, bottom: 0, left: 0 };

function memoryStorage(): Pick<Storage, 'getItem' | 'setItem'> & { writes: number } {
  const store = new Map<string, string>();
  return {
    writes: 0,
    getItem: (key) => store.get(key) ?? null,
    setItem(key, value) {
      this.writes += 1;
      store.set(key, value);
    },
  };
}

describe('foldSafeAreaReading', () => {
  it('publishes no floor while WebKit reports real insets', () => {
    expect(foldSafeAreaReading(undefined, PORTRAIT)).toEqual({ remembered: PORTRAIT, floor: null });
  });

  // The reported state: back from a phone call, every inset reads 0 and the
  // header sits under the clock.
  it('floors every side to the remembered insets once they all read 0', () => {
    expect(foldSafeAreaReading(PORTRAIT, LOST)).toEqual({ remembered: PORTRAIT, floor: PORTRAIT });
  });

  it('publishes no floor for a shape that never had insets', () => {
    expect(foldSafeAreaReading(undefined, LOST)).toEqual({ remembered: undefined, floor: null });
  });

  // Seen with a Live Activity in the Dynamic Island: the header under the
  // clock. A loss of the top alone must floor it too.
  it('floors the top when only the top drops to 0', () => {
    const topLost = { ...PORTRAIT, top: 0 };
    expect(foldSafeAreaReading(PORTRAIT, topLost)).toEqual({ remembered: PORTRAIT, floor: PORTRAIT });
  });

  it('floors the notch side when it drops to 0 in landscape', () => {
    const notchLost = { ...LANDSCAPE, left: 0 };
    expect(foldSafeAreaReading(LANDSCAPE, notchLost).floor).toEqual(LANDSCAPE);
  });

  // WebKit can pass through a half-lost reading on its way to the all-zero one.
  // Remembering it would floor the top to 0 for the rest of the session.
  it('never lets a half-lost reading replace the remembered insets', () => {
    const halfLost = foldSafeAreaReading(PORTRAIT, { ...PORTRAIT, top: 0 });
    expect(foldSafeAreaReading(halfLost.remembered, LOST).floor).toEqual(PORTRAIT);
  });

  // The bottom inset may drop on its own, for example while the keyboard is up.
  // Flooring it would open a gap above the keys.
  it('leaves a lone bottom zero alone', () => {
    const keyboardUp = { ...PORTRAIT, bottom: 0 };
    expect(foldSafeAreaReading(PORTRAIT, keyboardUp)).toEqual({ remembered: keyboardUp, floor: null });
  });

  // Mid-rotation, one frame can file one orientation's insets under the other's
  // shape. A real loss only takes insets away, so a reading that gains one is
  // real, and the memory was wrong. Kept, it would floor every later reading,
  // and storage would keep it across launches.
  it('replaces a memory that belongs to another shape', () => {
    expect(foldSafeAreaReading(PORTRAIT, LANDSCAPE)).toEqual({ remembered: LANDSCAPE, floor: null });
    expect(foldSafeAreaReading(LANDSCAPE, PORTRAIT)).toEqual({ remembered: PORTRAIT, floor: null });
  });

  it('follows a real change in an inset', () => {
    const taller = { ...PORTRAIT, top: 62 };
    expect(foldSafeAreaReading(PORTRAIT, taller)).toEqual({ remembered: taller, floor: null });
  });

  it('drops the floor as soon as WebKit reports insets again', () => {
    expect(foldSafeAreaReading(PORTRAIT, PORTRAIT).floor).toBeNull();
  });
});

describe('createSafeAreaMemory', () => {
  afterEach(() => vi.restoreAllMocks());

  // The PWA is often evicted and relaunched cold. A launch that starts in the
  // lost state must still find the insets an earlier launch read.
  it('floors a cold launch into the lost state from the insets a previous launch saw', () => {
    const storage = memoryStorage();
    createSafeAreaMemory(storage).set('portrait:393', PORTRAIT);

    const relaunched = createSafeAreaMemory(storage);
    expect(foldSafeAreaReading(relaunched.get('portrait:393'), LOST).floor).toEqual(PORTRAIT);
  });

  it('skips the write while the same shape keeps reading the same insets', () => {
    const storage = memoryStorage();
    const memory = createSafeAreaMemory(storage);
    memory.set('portrait:393', PORTRAIT);
    memory.set('portrait:393', { ...PORTRAIT });
    expect(storage.writes).toBe(1);
  });

  it('starts empty from unreadable or malformed storage', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    for (const raw of ['not json', '[]', 'null', '{"portrait:393":{"top":59}}', '{"portrait:393":{"top":-1,"right":0,"bottom":0,"left":0}}']) {
      const memory = createSafeAreaMemory({ getItem: () => raw, setItem: () => {} });
      expect(memory.get('portrait:393'), raw).toBeUndefined();
    }
    expect(warn).toHaveBeenCalledOnce();
  });

  it('keeps a valid shape beside a malformed one', () => {
    const raw = JSON.stringify({ 'portrait:393': PORTRAIT, 'landscape-primary:852': 'junk' });
    const memory = createSafeAreaMemory({ getItem: () => raw, setItem: () => {} });
    expect(memory.get('portrait:393')).toEqual(PORTRAIT);
    expect(memory.get('landscape-primary:852')).toBeUndefined();
  });

  it('still remembers for this launch when storage refuses the write', () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {});
    const memory = createSafeAreaMemory({
      getItem: () => null,
      setItem: () => { throw new Error('QuotaExceededError'); },
    });
    memory.set('portrait:393', PORTRAIT);
    expect(memory.get('portrait:393')).toEqual(PORTRAIT);
    expect(warn).toHaveBeenCalledOnce();
  });

  // An iPad window resize visits many widths, and each width is a shape.
  it('keeps only the most recently seen shapes', () => {
    const storage = memoryStorage();
    const memory = createSafeAreaMemory(storage);
    memory.set('portrait:393', PORTRAIT);
    for (let width = 500; width < 520; width += 1) memory.set(`portrait:${width}`, PORTRAIT);
    memory.set('portrait:519', { ...PORTRAIT, top: 24 });

    const relaunched = createSafeAreaMemory(storage);
    expect(relaunched.get('portrait:393')).toBeUndefined();
    expect(relaunched.get('portrait:519')).toEqual({ ...PORTRAIT, top: 24 });
  });

  it('counts a shape seen again as recent, even with unchanged insets', () => {
    const storage = memoryStorage();
    const memory = createSafeAreaMemory(storage);
    memory.set('portrait:393', PORTRAIT);
    for (let width = 500; width < 507; width += 1) memory.set(`portrait:${width}`, PORTRAIT);
    memory.set('portrait:393', PORTRAIT);
    memory.set('portrait:600', PORTRAIT);

    expect(createSafeAreaMemory(storage).get('portrait:393')).toEqual(PORTRAIT);
  });
});

describe('viewportShapeKey', () => {
  it('tells the two landscape sides apart, since the notch moves with them', () => {
    const left = viewportShapeKey({ orientation: 'landscape-primary', width: 852, height: 393 });
    const right = viewportShapeKey({ orientation: 'landscape-secondary', width: 852, height: 393 });
    expect(left).not.toBe(right);
  });

  it('falls back to the aspect ratio without screen.orientation', () => {
    expect(viewportShapeKey({ orientation: undefined, width: 393, height: 852 })).toBe('portrait:393');
    expect(viewportShapeKey({ orientation: undefined, width: 852, height: 393 })).toBe('landscape:852');
  });

  it('ignores a height change, which the keyboard can cause', () => {
    const open = viewportShapeKey({ orientation: 'portrait-primary', width: 393, height: 500 });
    const closed = viewportShapeKey({ orientation: 'portrait-primary', width: 393, height: 852 });
    expect(open).toBe(closed);
  });
});
