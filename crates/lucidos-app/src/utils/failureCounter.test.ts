import { describe, it, expect, vi } from 'vitest';
import { createFailureCounter } from './failureCounter';

describe('createFailureCounter', () => {
  it('does not fire onThreshold below the threshold', () => {
    const onThreshold = vi.fn();
    const counter = createFailureCounter(3, onThreshold);
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).not.toHaveBeenCalled();
  });

  it('fires onThreshold exactly once when the count reaches the threshold', () => {
    const onThreshold = vi.fn();
    const counter = createFailureCounter(3, onThreshold);
    counter.recordFailure();
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).toHaveBeenCalledTimes(1);
  });

  it('does not re-fire onThreshold while past the threshold', () => {
    const onThreshold = vi.fn();
    const counter = createFailureCounter(2, onThreshold);
    counter.recordFailure();
    counter.recordFailure();
    counter.recordFailure();
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).toHaveBeenCalledTimes(1);
  });

  it('recordSuccess resets the counter and the notified flag', () => {
    const onThreshold = vi.fn();
    const counter = createFailureCounter(2, onThreshold);
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).toHaveBeenCalledTimes(1);
    counter.recordSuccess();
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).toHaveBeenCalledTimes(2);
  });

  it('recordSuccess before threshold cancels the pending escalation', () => {
    const onThreshold = vi.fn();
    const counter = createFailureCounter(3, onThreshold);
    counter.recordFailure();
    counter.recordFailure();
    counter.recordSuccess();
    counter.recordFailure();
    counter.recordFailure();
    expect(onThreshold).not.toHaveBeenCalled();
  });

  it('throws on a non-positive threshold (rejecting silently would mean never firing)', () => {
    expect(() => createFailureCounter(0, () => {})).toThrow();
    expect(() => createFailureCounter(-1, () => {})).toThrow();
  });
});
