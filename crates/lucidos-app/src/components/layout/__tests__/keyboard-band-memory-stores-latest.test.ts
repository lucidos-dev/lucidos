import { describe, it, expect } from 'vitest';
import { keyboardBandMemory } from '../MobileSwipeContainer';

const portrait = { width: 390, height: 844 };

function memoryStorage() {
  const items = new Map<string, string>();
  return {
    getItem: (key: string) => items.get(key) ?? null,
    setItem: (key: string, value: string) => { items.set(key, value); },
  };
}

describe('the keyboard band memory stores the latest measurement', () => {
  it('reserves the latest stored band on the next launch, not the widest seen', () => {
    const storage = memoryStorage();
    const first = keyboardBandMemory(storage);
    first.note(336, portrait);
    first.note(300, portrait);

    const nextLaunch = keyboardBandMemory(storage);
    expect(nextLaunch.reserve(portrait)).toBe(300);
  });
});
