/** An `ImageUploaded` from the event stream records the hash the engine
 *  stored, which lands the matching pending image. Plan:
 *  `docs/plans/2026-10-04-image-upload-lands-on-its-event.md`. */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';

import type { ThreadEventRow } from '../../api/threads';
import { landedImages, _resetLandedImagesForTesting } from '../landedImages';
import { threadMap } from '../store';
import { makeOptimisticThreadState } from '../thread-events';
import { _applyEventRowsForTest } from './thread-loading';
import { handleThreadEvent } from './thread-sync';

const THREAD = 't-1';

describe('handleThreadEvent: ImageUploaded', () => {
  beforeEach(() => {
    _resetLandedImagesForTesting();
    threadMap.value = new Map([[THREAD, makeOptimisticThreadState({
      id: THREAD, title: '', channel: 'chat', initiator: 'user',
      eventsLoaded: true, state: 'composing', status: 'idle',
    })]]);
  });
  afterEach(() => {
    threadMap.value = new Map();
    _resetLandedImagesForTesting();
  });

  it('records the stored hash against its thread', () => {
    handleThreadEvent({
      thread_id: THREAD,
      seq: null,
      event: { type: 'ImageUploaded', hash: 'h1', mime: 'image/png', byte_size: 3 },
    });
    expect([...(landedImages.value.get(THREAD) ?? [])]).toEqual(['h1']);
  });

  it('records nothing for any other event', () => {
    handleThreadEvent({
      thread_id: THREAD,
      seq: null,
      event: { type: 'ThreadTitleGenerated', title: 'A title' },
    });
    expect(landedImages.value.size).toBe(0);
  });

  it('records a hash from replayed history too: a reload, or a catch-up after a gap', () => {
    const map = threadMap.value;
    _applyEventRowsForTest(map, map.get(THREAD)!, [{
      event_id: 'e-1',
      event_type: 'ImageUploaded',
      payload: { hash: 'h1', mime: 'image/png', byte_size: 3 },
      sequence: 1,
      created: '2026-01-01T00:00:00Z',
    } as ThreadEventRow]);
    expect([...(landedImages.value.get(THREAD) ?? [])]).toEqual(['h1']);
  });
});
