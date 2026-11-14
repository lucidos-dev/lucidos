import { describe, it, expect, vi, beforeEach } from 'vitest';
import {
  MAX_FIT_FINDINGS,
  WIDGET_CONTENT_SIZE_MESSAGE_TYPE,
  WIDGET_FIT_MESSAGE_TYPE,
  WIDGET_MENU_MESSAGE_TYPE,
} from '@lucidos/widget-frame';
import { overlayStack, pushOverlay } from '../overlayStack';
import { primaryPointerIsDown } from '../../utils/pointerPress';

const appFrameFor = vi.fn();
vi.mock('../../utils/appFrame', () => ({ appFrameFor }));

const {
  handleWidgetFrameMessage, APP_FRAME_FIT_EVENT, APP_FRAME_MENU_EVENT, APP_FRAME_SIZE_EVENT,
} = await import('./widget-frame-bridge');

const SOURCE = { name: 'the dashboard tile' };
const message = (data: unknown, source: unknown = SOURCE) => ({ data, source }) as unknown as MessageEvent;

describe('handleWidgetFrameMessage', () => {
  let frame: { dispatchEvent: ReturnType<typeof vi.fn>; getBoundingClientRect: () => DOMRect; closest: () => unknown };
  let panel: string | null;
  const dispatched = () => frame.dispatchEvent.mock.calls[0][0] as CustomEvent;

  beforeEach(() => {
    frame = {
      dispatchEvent: vi.fn(),
      getBoundingClientRect: () => ({ left: 100, top: 200, right: 400, bottom: 360 }) as DOMRect,
      // The overlay panel the frame sits in, if any.
      closest: () => (panel === null ? null : { getAttribute: () => panel }),
    };
    panel = null;
    overlayStack.value = [];
    // A blur forgets any press an earlier case marked.
    window.dispatchEvent(new Event('blur'));
    appFrameFor.mockReset().mockImplementation((s) => (s === SOURCE ? frame : null));
  });

  it('dispatches the height on the frame that sent it', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 312 }));
    expect(dispatched().type).toBe(APP_FRAME_SIZE_EVENT);
    expect(dispatched().detail).toEqual({ height: 312 });
  });

  it('carries the width, so the box can fade a sideways spill', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 120, width: 180 }));
    expect(dispatched().detail).toEqual({ height: 120, width: 180 });
  });

  it('drops a width that is not a length', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 120, width: -4 }));
    expect(dispatched().detail).toEqual({ height: 120 });
  });

  it('carries the laid-out widget size', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 120, size: 'large' }));
    expect(dispatched().detail).toEqual({ height: 120, size: 'large' });
  });

  it('drops a laid-out size that is not a widget size', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 120, size: 'huge' }));
    expect(dispatched().detail).toEqual({ height: 120 });
  });

  it('dispatches a menu request at the point in the viewport', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at: { x: 40, y: 24 } }));
    expect(dispatched().type).toBe(APP_FRAME_MENU_EVENT);
    expect(dispatched().bubbles).toBe(true);
    expect(dispatched().detail).toEqual({ x: 140, y: 224 });
  });

  it('clamps a point into the frame that sent it', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at: { x: -50, y: 5000 } }));
    expect(dispatched().detail).toEqual({ x: 100, y: 360 });
  });

  it('dispatches a menu request with no point, as a held finger sends', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, held: true }));
    expect(dispatched().type).toBe(APP_FRAME_MENU_EVENT);
    expect(dispatched().detail).toBeNull();
  });

  it('marks a held finger down before the menu opens, so the menu spends its lift', () => {
    let downAtOpen: boolean | null = null;
    frame.dispatchEvent.mockImplementation(() => { downAtOpen = primaryPointerIsDown(); return true; });
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, held: true }));
    expect(downAtOpen).toBe(true);
  });

  it('marks no press for a right-click, which brings no click', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at: { x: 40, y: 24 } }));
    expect(primaryPointerIsDown()).toBe(false);
  });

  it('drops a menu request whose point is not two finite numbers', () => {
    for (const at of [{ x: '40', y: 24 }, { x: Number.NaN, y: 1 }, { x: 1 }, 'here']) {
      handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at }));
    }
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('drops a menu request from behind an open overlay panel, so one menu shows at a time', () => {
    pushOverlay({ id: 'a menu', dismiss: () => {}, hasPanel: true });
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at: { x: 40, y: 24 } }));
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('takes a menu request from a frame inside the top panel', () => {
    pushOverlay({ id: 'the top panel', dismiss: () => {}, hasPanel: true });
    panel = 'the top panel';
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE, at: { x: 40, y: 24 } }));
    expect(dispatched().type).toBe(APP_FRAME_MENU_EVENT);
  });

  it('ignores a window that is not a mounted app frame', () => {
    handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height: 312 }, { name: 'an embed' }));
    handleWidgetFrameMessage(message({ type: WIDGET_MENU_MESSAGE_TYPE }, { name: 'an embed' }));
    expect(frame.dispatchEvent).not.toHaveBeenCalled();
  });

  it('ignores a height that is not a finite, non-negative number', () => {
    for (const height of ['312', -1, Number.NaN, Number.POSITIVE_INFINITY, undefined]) {
      handleWidgetFrameMessage(message({ type: WIDGET_CONTENT_SIZE_MESSAGE_TYPE, height }));
    }
    expect(appFrameFor).not.toHaveBeenCalled();
  });

  describe('a fit report', () => {
    const finding = { kind: 'ellipsis', edge: 'right', px: 42, path: 'div.row > span', text: 'Oslo to Lisbon' };
    const report = (extra: Record<string, unknown> = {}) => ({
      type: WIDGET_FIT_MESSAGE_TYPE, token: 't1', size: 'large', box: { width: 320, height: 320 }, findings: [finding], total: 1, ...extra,
    });

    it('dispatches the box and findings on the frame that measured', () => {
      handleWidgetFrameMessage(message(report()));
      expect(dispatched().type).toBe(APP_FRAME_FIT_EVENT);
      expect(dispatched().bubbles).toBe(true);
      expect(dispatched().detail).toEqual({ token: 't1', size: 'large', box: { width: 320, height: 320 }, findings: [finding], total: 1 });
    });

    it('dispatches why a frame could not measure', () => {
      handleWidgetFrameMessage(message({ type: WIDGET_FIT_MESSAGE_TYPE, token: 't1', error: 'boom' }));
      expect(dispatched().detail).toEqual({ token: 't1', error: 'boom' });
    });

    it('keeps at most the findings a frame may send', () => {
      handleWidgetFrameMessage(message(report({ findings: Array(MAX_FIT_FINDINGS + 3).fill(finding), total: 11 })));
      expect(dispatched().detail.findings).toHaveLength(MAX_FIT_FINDINGS);
      expect(dispatched().detail.total).toBe(11);
    });

    it('drops a report with no token, no box, or a malformed finding', () => {
      for (const bad of [
        report({ token: '' }),
        report({ token: 7 }),
        report({ box: undefined }),
        report({ box: { width: -1, height: 2 } }),
        report({ findings: 'none' }),
        report({ findings: [{ ...finding, kind: 'odd' }] }),
        report({ findings: [{ ...finding, edge: 'middle' }] }),
        report({ findings: [{ ...finding, px: Number.NaN }] }),
        report({ findings: [{ ...finding, text: 3 }] }),
        report({ total: -1 }),
        { type: WIDGET_FIT_MESSAGE_TYPE, token: 't1', error: 5 },
      ]) handleWidgetFrameMessage(message(bad));
      expect(frame.dispatchEvent).not.toHaveBeenCalled();
    });

    it('drops a size that is not a widget size', () => {
      handleWidgetFrameMessage(message(report({ size: 'huge' })));
      expect(dispatched().detail.size).toBeUndefined();
    });
  });
});
