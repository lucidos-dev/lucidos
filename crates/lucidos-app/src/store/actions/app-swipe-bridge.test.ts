import { describe, it, expect, vi, beforeEach } from 'vitest';
import { APP_SWIPE_END_MESSAGE_TYPE, APP_SWIPE_MESSAGE_TYPE } from '@lucidos/pane-swipe';

const isKnownAppFrame = vi.fn();
vi.mock('../../utils/appFrame', () => ({ isKnownAppFrame }));

const { parseAppSwipeMessage } = await import('./app-swipe-bridge');

const APP = { name: 'the app frame' };
const message = (data: unknown, source: unknown = APP) => ({ data, source }) as unknown as MessageEvent;

describe('parseAppSwipeMessage', () => {
  beforeEach(() => {
    isKnownAppFrame.mockReset().mockImplementation((s) => s === APP);
  });

  it('reads a drag', () => {
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_MESSAGE_TYPE, dx: -40 }))).toEqual({ kind: 'drag', dx: -40 });
  });

  it('drops a drag no gesture could produce', () => {
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_MESSAGE_TYPE, dx: 'far' }))).toBeNull();
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_MESSAGE_TYPE, dx: Number.NaN }))).toBeNull();
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_MESSAGE_TYPE, dx: Infinity }))).toBeNull();
  });

  it('reads a release to either neighbour', () => {
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta: 1 }))).toEqual({ kind: 'release', paneDelta: 1 });
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta: -1 }))).toEqual({ kind: 'release', paneDelta: -1 });
  });

  it('reads a release no gesture could produce as a snap back, so the drag still ends', () => {
    for (const paneDelta of [2, -5, 'next', undefined]) {
      expect(parseAppSwipeMessage(message({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta }))).toEqual({ kind: 'release', paneDelta: 0 });
    }
  });

  it('ignores a frame that is not a current app frame, such as a nested embed', () => {
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_MESSAGE_TYPE, dx: -40 }, { name: 'an embed' }))).toBeNull();
    expect(parseAppSwipeMessage(message({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta: 1 }, { name: 'an embed' }))).toBeNull();
  });

  it('ignores every other message without looking up the frame', () => {
    expect(parseAppSwipeMessage(message({ type: 'lucidos:app:pull', travel: 40 }))).toBeNull();
    expect(parseAppSwipeMessage(message(null))).toBeNull();
    expect(isKnownAppFrame).not.toHaveBeenCalled();
  });
});
