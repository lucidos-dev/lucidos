import { describe, it, expect, vi, beforeEach } from 'vitest';
import { WIDGET_SIZE_MESSAGE_TYPE, WIDGET_TAP_MESSAGE_TYPE } from '@lucidos/widget-frame';

const appFrameFor = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor }));

const { handleWidgetFrameMessage, APP_FRAME_SIZE_EVENT, APP_FRAME_TAP_EVENT } = await import('./widget-frame-bridge');

const SOURCE = { name: 'the widget window' };
const message = (data: unknown, source: unknown = SOURCE) => ({ data, source }) as unknown as MessageEvent;

describe('handleWidgetFrameMessage', () => {
  let frame: { dispatchEvent: ReturnType<typeof vi.fn> };
  const dispatched = () => frame.dispatchEvent.mock.calls[0][0] as CustomEvent;

  beforeEach(() => {
    frame = { dispatchEvent: vi.fn() };
    appFrameFor.mockReset().mockImplementation((s) => (s === SOURCE ? frame : null));
  });

  it('dispatches the height on the frame that sent it', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_SIZE_MESSAGE_TYPE, height: 312 }));
    expect(dispatched().type).toBe(APP_FRAME_SIZE_EVENT);
    expect(dispatched().detail).toEqual({ height: 312 });
  });

  it('passes a minimal frame\'s width through', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_SIZE_MESSAGE_TYPE, height: 120, width: 180 }));
    expect(dispatched().detail).toEqual({ height: 120, width: 180 });
  });

  it('dispatches a press with its pointer type', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: 'touch' }));
    expect(dispatched().type).toBe(APP_FRAME_TAP_EVENT);
    expect(dispatched().detail).toBe('touch');
  });

  it('ignores a window that is not a mounted app frame', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_SIZE_MESSAGE_TYPE, height: 312 }, { name: 'an embed' }));
    handleWidgetFrameMessage(message({ type: WIDGET_TAP_MESSAGE_TYPE, pointerType: 'touch' }, { name: 'an embed' }));
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('ignores a height or width that is not a finite, non-negative number', () => {
    for (const height of ['312', -1, Number.NaN, Number.POSITIVE_INFINITY, undefined]) {
      handleWidgetFrameMessage(message({ type: WIDGET_SIZE_MESSAGE_TYPE, height }));
    }
    handleWidgetFrameMessage(message({ type: WIDGET_SIZE_MESSAGE_TYPE, height: 10, width: -4 }));
    expect(appFrameFor).not.toHaveBeenCalled();
  });
});
