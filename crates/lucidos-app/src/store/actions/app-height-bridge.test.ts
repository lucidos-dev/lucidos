import { describe, it, expect, vi, beforeEach } from 'vitest';
import { WIDGET_HEIGHT_MESSAGE_TYPE } from '@lucidos/widget-height';

const appFrameFor = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor }));

const { handleAppHeightMessage, APP_FRAME_HEIGHT_EVENT } = await import('./app-height-bridge');

const SOURCE = { name: 'the widget window' };
const message = (data: unknown, source: unknown = SOURCE) => ({ data, source }) as unknown as MessageEvent;

describe('handleAppHeightMessage', () => {
  let frame: { dispatchEvent: ReturnType<typeof vi.fn> };

  beforeEach(() => {
    frame = { dispatchEvent: vi.fn() };
    appFrameFor.mockReset().mockImplementation((s) => (s === SOURCE ? frame : null));
  });

  it('dispatches the height on the frame that sent it', () => {
    handleAppHeightMessage(message({ type: WIDGET_HEIGHT_MESSAGE_TYPE, height: 312 }));
    const event = frame.dispatchEvent.mock.calls[0][0] as CustomEvent<number>;
    expect(event.type).toBe(APP_FRAME_HEIGHT_EVENT);
    expect(event.detail).toBe(312);
  });

  it('ignores a window that is not a mounted app frame', () => {
    handleAppHeightMessage(message({ type: WIDGET_HEIGHT_MESSAGE_TYPE, height: 312 }, { name: 'an embed' }));
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('ignores a height that is not a finite, non-negative number', () => {
    for (const height of ['312', -1, Number.NaN, Number.POSITIVE_INFINITY, undefined]) {
      handleAppHeightMessage(message({ type: WIDGET_HEIGHT_MESSAGE_TYPE, height }));
    }
    expect(appFrameFor).not.toHaveBeenCalled();
  });
});
