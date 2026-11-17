import { expect, it, vi } from 'vitest';
import { noteCrashWorker, onCrashWorker } from './crashWorker';
import { noteCrashPageHide, onCrashPageHide } from './crashPageHide';

it('isolates a failed diagnostic listener from lifecycle handling', () => {
  const fail = () => { throw new Error('storage unavailable'); };
  const worker = vi.fn();
  const hide = vi.fn();
  const stops = [onCrashWorker(fail), onCrashWorker(worker), onCrashPageHide(fail), onCrashPageHide(hide)];
  try {
    expect(() => noteCrashWorker(4)).not.toThrow();
    expect(() => noteCrashPageHide(true)).not.toThrow();
    expect(worker).toHaveBeenCalledWith(4);
    expect(hide).toHaveBeenCalledWith(2);
  } finally { stops.forEach(stop => stop()); }
});
