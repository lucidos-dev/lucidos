import { describe, expect, it } from 'vitest';
import { snapshotCaptureFailureMessage } from './snapshot-capture-failure';

describe('snapshotCaptureFailureMessage', () => {
  it('names the space to free for a disk floor refusal', () => {
    expect(snapshotCaptureFailureMessage({ 'disk-floor': { needed_bytes: 12 * 1024 ** 3 } }))
      .toBe('Not enough free disk space for a snapshot. Free up 12.0 GB and try again.');
  });

  it('rounds the space to free up, so freeing the named amount clears the floor', () => {
    expect(snapshotCaptureFailureMessage({ 'disk-floor': { needed_bytes: 12.04 * 1024 ** 3 } }))
      .toBe('Not enough free disk space for a snapshot. Free up 12.1 GB and try again.');
  });

  it.each(['capture-failed', 'interrupted', null, undefined, {}, { 'disk-floor': {} }])(
    'keeps the generic copy for %j',
    reason => {
      expect(snapshotCaptureFailureMessage(reason))
        .toBe('Snapshot capture failed. Review snapshot status before trying again.');
    },
  );
});
