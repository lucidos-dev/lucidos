// @vitest-environment jsdom
/**
 * A widget frame forwards its right-click only when its surface opens a menu
 * for it (plan invariants I1 and I5).
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { WIDGET_CHANNEL } from '@lucidos/widget-frame';

const { postToAppFrame } = vi.hoisted(() => ({ postToAppFrame: vi.fn() }));
vi.mock('../../../store/actions/app-bridge', () => ({ postToAppFrame }));

import { WidgetFrame } from '../WidgetFrame';
import { APP_FRAME_MENU_EVENT } from '../../../store/actions/widget-frame-bridge';

let host: HTMLElement;

beforeEach(() => {
  postToAppFrame.mockReset();
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const loadedFrame = () => {
  const frame = host.querySelector('iframe')!;
  act(() => { frame.dispatchEvent(new Event('load')); });
  return frame;
};
const lastPush = () => {
  const pushes = postToAppFrame.mock.calls.filter((c) => c[1] === WIDGET_CHANNEL);
  return pushes[pushes.length - 1]?.[2];
};

describe('WidgetFrame and the widget menu', () => {
  it('marks the frame for a menu only when it has somewhere to open one', () => {
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" />, host); });
    loadedFrame();
    expect(lastPush()).not.toHaveProperty('menu');

    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" onMenu={() => {}} />, host); });
    expect(lastPush()).toMatchObject({ place: 'shelf', menu: true });
  });

  it("hands the frame's menu request to onMenu, with its point or none", () => {
    const onMenu = vi.fn();
    act(() => { render(<WidgetFrame appId="fare-grid" reveal="on-load" place="shelf" onMenu={onMenu} />, host); });
    const frame = loadedFrame();
    frame.dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { detail: { x: 140, y: 224 }, bubbles: true }));
    frame.dispatchEvent(new CustomEvent(APP_FRAME_MENU_EVENT, { bubbles: true }));
    expect(onMenu.mock.calls).toEqual([[{ x: 140, y: 224 }], [undefined]]);
  });
});
