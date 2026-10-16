import { describe, it, expect, vi, beforeEach } from 'vitest';
import { APP_SCROLL_MESSAGE_TYPE, APP_TOUCH_MESSAGE_TYPE } from '@lucidos/app-scroll';

const CANVAS_FRAME = { canvas: true };
const WIDGET_FRAME = { canvas: false };
const appFrameFor = vi.fn();
const isCanvasAppFrame = (frame: { canvas: boolean } | null) => frame?.canvas === true;
vi.mock('../../utils/appFrame', () => ({ appFrameFor, isCanvasAppFrame }));

const { parseAppScrollMessage, parseAppTouchMessage } = await import('./app-scroll-bridge');

const APP = { name: 'the app frame' };
const WIDGET = { name: 'a shelf widget' };
const STRANGER = { name: 'an embed inside the app' };
const message = (data: unknown, source: unknown = APP) => ({ data, source }) as unknown as MessageEvent;
const report = { type: APP_SCROLL_MESSAGE_TYPE, delta: 30, top: 400, max: 2000 };

describe('parseAppScrollMessage', () => {
  beforeEach(() => {
    appFrameFor.mockReset().mockImplementation((s) =>
      s === APP ? CANVAS_FRAME : s === WIDGET ? WIDGET_FRAME : null);
  });

  it("reads the Canvas app's scroll, with the frame it came from", () => {
    expect(parseAppScrollMessage(message(report))).toEqual({ source: APP, delta: 30, top: 400, max: 2000 });
  });

  it('ignores a widget in a thread and an embed inside the app', () => {
    expect(parseAppScrollMessage(message(report, WIDGET))).toBeNull();
    expect(parseAppScrollMessage(message(report, STRANGER))).toBeNull();
  });

  it('ignores another message type, and a report with a field that is not a number', () => {
    expect(parseAppScrollMessage(message({ ...report, type: 'lucidos:app:pull' }))).toBeNull();
    expect(parseAppScrollMessage(message({ ...report, delta: '30' }))).toBeNull();
    expect(parseAppScrollMessage(message({ ...report, top: Number.NaN }))).toBeNull();
    expect(parseAppScrollMessage(message(null))).toBeNull();
  });
});

describe('parseAppTouchMessage', () => {
  const touch = { type: APP_TOUCH_MESSAGE_TYPE, touching: true };

  beforeEach(() => {
    appFrameFor.mockReset().mockImplementation((s) =>
      s === APP ? CANVAS_FRAME : s === WIDGET ? WIDGET_FRAME : null);
  });

  it("reads whether a finger is on the Canvas app, with the frame it came from", () => {
    expect(parseAppTouchMessage(message(touch))).toEqual({ source: APP, touching: true });
    expect(parseAppTouchMessage(message({ ...touch, touching: false }))).toEqual({ source: APP, touching: false });
  });

  it('ignores a widget in a thread and an embed inside the app', () => {
    expect(parseAppTouchMessage(message(touch, WIDGET))).toBeNull();
    expect(parseAppTouchMessage(message(touch, STRANGER))).toBeNull();
  });

  it('ignores a scroll report, and a touch that is not a boolean', () => {
    expect(parseAppTouchMessage(message(report))).toBeNull();
    expect(parseAppTouchMessage(message({ ...touch, touching: 1 }))).toBeNull();
  });
});
