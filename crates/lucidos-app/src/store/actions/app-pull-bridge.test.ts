import { describe, it, expect, vi, beforeEach } from 'vitest';
import { APP_PULL_MESSAGE_TYPE, APP_REFRESH_MESSAGE_TYPE } from '@lucidos/pull-to-refresh';

const CANVAS_FRAME = { canvas: true };
const WIDGET_FRAME = { canvas: false };
const appFrameFor = vi.fn();
const isCanvasAppFrame = (frame: { canvas: boolean } | null) => frame?.canvas === true;
const runPanelRefresh = vi.fn();
const showPullTravel = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor, isCanvasAppFrame }));
vi.mock('../panelRefresh', () => ({ runPanelRefresh, showPullTravel }));

const { handleAppPullMessage } = await import('./app-pull-bridge');

const APP = { name: 'the app frame' };
const WIDGET = { name: 'a shelf widget' };
const message = (data: unknown, source: unknown = APP) => ({ data, source }) as unknown as MessageEvent;

describe('handleAppPullMessage', () => {
  beforeEach(() => {
    appFrameFor.mockReset().mockImplementation((s) =>
      s === APP ? CANVAS_FRAME : s === WIDGET ? WIDGET_FRAME : null);
    runPanelRefresh.mockReset();
    showPullTravel.mockReset();
  });

  it('ignores a pull from a shelf widget, which is not the Canvas app', () => {
    handleAppPullMessage(message({ type: APP_REFRESH_MESSAGE_TYPE }, WIDGET));
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: 40 }, WIDGET));
    expect(runPanelRefresh).not.toHaveBeenCalled();
    expect(showPullTravel).not.toHaveBeenCalled();
  });

  it('runs the app panel\'s refresh on a release', () => {
    handleAppPullMessage(message({ type: APP_REFRESH_MESSAGE_TYPE }));
    expect(runPanelRefresh).toHaveBeenCalledTimes(1);
  });

  it('moves the affordance with the pull', () => {
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: 40 }));
    expect(showPullTravel).toHaveBeenCalledWith(40);
  });

  it('passes a long pull through, since the affordance caps its own position', () => {
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: 10_000 }));
    expect(showPullTravel).toHaveBeenLastCalledWith(10_000);
  });

  it('reads a travel no gesture could produce as no pull', () => {
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: 'far' }));
    expect(showPullTravel).toHaveBeenLastCalledWith(0);
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: -40 }));
    expect(showPullTravel).toHaveBeenLastCalledWith(0);
  });

  it('ignores a frame that is not a current app frame, such as a nested embed', () => {
    handleAppPullMessage(message({ type: APP_REFRESH_MESSAGE_TYPE }, { name: 'an embed' }));
    handleAppPullMessage(message({ type: APP_PULL_MESSAGE_TYPE, travel: 40 }, { name: 'an embed' }));
    expect(runPanelRefresh).not.toHaveBeenCalled();
    expect(showPullTravel).not.toHaveBeenCalled();
  });

  it('ignores every other message', () => {
    handleAppPullMessage(message({ type: 'lucidos:ui:toast' }));
    handleAppPullMessage(message(null));
    expect(appFrameFor).not.toHaveBeenCalled();
    expect(runPanelRefresh).not.toHaveBeenCalled();
  });
});
