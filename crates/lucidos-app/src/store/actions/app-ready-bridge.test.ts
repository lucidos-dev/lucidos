import { describe, it, expect, vi, beforeEach } from 'vitest';

const appFrameFor = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor }));

const { handleAppReadyMessage, APP_READY_MESSAGE_TYPE, APP_FRAME_READY_EVENT } = await import('./app-ready-bridge');

const SOURCE = { name: 'the app window' };
const message = (data: unknown, source: unknown = SOURCE) => ({ data, source }) as unknown as MessageEvent;

describe('handleAppReadyMessage', () => {
  let frame: { dispatchEvent: ReturnType<typeof vi.fn> };

  beforeEach(() => {
    frame = { dispatchEvent: vi.fn() };
    appFrameFor.mockReset().mockImplementation((s) => (s === SOURCE ? frame : null));
  });

  it('dispatches the ready event on the frame that sent it', () => {
    handleAppReadyMessage(message({ type: APP_READY_MESSAGE_TYPE }));
    expect(frame.dispatchEvent).toHaveBeenCalledTimes(1);
    expect((frame.dispatchEvent.mock.calls[0][0] as Event).type).toBe(APP_FRAME_READY_EVENT);
  });

  it('ignores a window that is not a mounted app frame, such as a nested embed', () => {
    handleAppReadyMessage(message({ type: APP_READY_MESSAGE_TYPE }, { name: 'an embed' }));
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('ignores every other message', () => {
    handleAppReadyMessage(message({ type: 'lucidos:ui:toast' }));
    handleAppReadyMessage(message(null));
    handleAppReadyMessage(message('lucidos:ui:ready'));
    expect(appFrameFor).not.toHaveBeenCalled();
  });
});
