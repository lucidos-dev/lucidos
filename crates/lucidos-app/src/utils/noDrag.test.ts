import { describe, it, expect } from 'vitest';
import { shouldSuppressDragStart } from './noDrag';

// A minimal element stand-in: `closest` returns a truthy match when the target
// is inside an opted-in drag source, else null — mirroring DOM `closest`.
const elem = (match: unknown) => ({ closest: (_sel: string) => match });

describe('shouldSuppressDragStart', () => {
  it('suppresses a drag from ordinary content (no draggable opt-in)', () => {
    expect(shouldSuppressDragStart(elem(null) as unknown as EventTarget)).toBe(true);
  });

  it('allows a drag from an element opted in via draggable="true"', () => {
    expect(shouldSuppressDragStart(elem({}) as unknown as EventTarget)).toBe(false);
  });

  it('suppresses a non-element target (text node / document / null)', () => {
    expect(shouldSuppressDragStart(null)).toBe(true);
    expect(shouldSuppressDragStart({} as EventTarget)).toBe(true);
  });
});
