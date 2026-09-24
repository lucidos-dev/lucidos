import { describe, it, expect } from 'vitest';
import { toastPressVerdict } from './toastPressProbe';
import source from './toastPressProbe.ts?raw';

const base = { lifted: true, touchCancelled: false, clicked: false, clickPrevented: false };

describe('toastPressVerdict', () => {
  it('reads a click nobody cancelled as clicked', () => {
    expect(toastPressVerdict({ ...base, clicked: true })).toBe('clicked');
  });

  it('reads a cancelled click as click-cancelled, which is what a swallow leaves', () => {
    expect(toastPressVerdict({ ...base, clicked: true, clickPrevented: true })).toBe('click-cancelled');
  });

  it('reads a lift with no click after it as no-click', () => {
    expect(toastPressVerdict(base)).toBe('no-click');
  });

  it('reads a gesture the system took as touch-cancelled', () => {
    expect(toastPressVerdict({ ...base, lifted: false, touchCancelled: true })).toBe('touch-cancelled');
  });

  it('reads a press whose lift never came as no-lift', () => {
    expect(toastPressVerdict({ ...base, lifted: false })).toBe('no-lift');
  });
});

describe('the probe takes no gesture', () => {
  it('never cancels or stops an event', () => {
    expect(source).not.toMatch(/\.preventDefault\(/);
    expect(source).not.toMatch(/\.stopPropagation\(/);
  });

  it('never labels a press with the text of the toast body, which is the message', () => {
    expect(source).toMatch(/if \(el\.tagName !== 'BUTTON'\) return 'toast body';/);
  });

  it('registers every listener passive', () => {
    const adds = source.match(/addEventListener\([^)]*\)/g) ?? [];
    expect(adds.length).toBe(5);
    for (const add of adds) expect(add).toContain('passiveCapture');
  });
});
