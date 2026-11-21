// @vitest-environment jsdom
/**
 * The widget fit check renders the real widget box at every size, on the desk
 * tile and the phone column, at the check's UI scale. It answers once.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { SETTLE_CAP_MS, WIDGET_CHANNEL } from '@lucidos/widget-frame';

const { postToAppFrame, postWidgetFitReportApi } = vi.hoisted(() => ({
  postToAppFrame: vi.fn(),
  postWidgetFitReportApi: vi.fn((_requestId: string, _boxes: Array<Record<string, unknown>>) => Promise.resolve({ ok: true })),
}));
vi.mock('../../../store/actions/app-bridge', () => ({ postToAppFrame }));
vi.mock('../../../api/client/widgets', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client/widgets')>()),
  postWidgetFitReportApi,
}));

import { WidgetFitCheck, FIT_ANSWER_SLACK_MS, PHONE_COLUMN_PX } from '../WidgetFitCheck';
import { requestWidgetFit, widgetFitRequests } from '../../../store/widgetFit';
import { APP_FRAME_FIT_EVENT, type WidgetFit } from '../../../store/actions/widget-frame-bridge';
import { WIDGET_FIT_SCALE_ATTRIBUTE } from '../../../store/actions/app-appearance';
import { DASHBOARD_CELL_REM, widgetCellPx } from '../../../utils/dashboardGeometry';
import { DEVICE_ID_KEY } from '../../../utils/deviceIdHeader';

let host: HTMLElement;

beforeEach(() => {
  postToAppFrame.mockReset();
  postWidgetFitReportApi.mockClear();
  widgetFitRequests.value = [];
  document.documentElement.style.fontSize = '16px';
  vi.stubGlobal('requestAnimationFrame', (run: FrameRequestCallback) => { run(performance.now()); return 0; });
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<WidgetFitCheck />, host); });
});

afterEach(() => {
  vi.unstubAllGlobals();
  vi.useRealTimers();
  render(null, host);
  host.remove();
  document.documentElement.style.fontSize = '';
});

const surface = (name: string) => host.querySelector<HTMLElement>(`.widget-fit-surface[data-surface="${name}"]`);
const frameOn = (name: string) => surface(name)?.querySelector('iframe') ?? null;
/** The token the frame on `name` was last asked to measure for. */
const lastMeasure = (name: string): string | undefined => {
  const frame = frameOn(name);
  const pushes = postToAppFrame.mock.calls.filter((c) => c[0] === frame && c[1] === WIDGET_CHANNEL);
  return pushes[pushes.length - 1]?.[2].measure;
};
const fits = (token: string): WidgetFit => ({ token, box: { width: 1, height: 1 }, findings: [], total: 0 });

/** `scaleDevice` null: the workspace-wide scale is the largest. */
function check(sizes: Array<'small' | 'medium' | 'large' | 'xl'>, scaleDevice: string | null = 'the-phone') {
  act(() => { requestWidgetFit({ requestId: 'r1', appId: 'trip', sizes, scale: 125, scaleDevice: scaleDevice ?? undefined }); });
  for (const name of ['desk-tile', 'phone-column']) act(() => { frameOn(name)!.dispatchEvent(new Event('load')); });
}

/** Answer the size the frame on `name` measures now. */
function answer(name: string, fit: (token: string) => WidgetFit = fits) {
  const token = lastMeasure(name)!;
  act(() => { frameOn(name)!.dispatchEvent(new CustomEvent(APP_FRAME_FIT_EVENT, { detail: fit(token), bubbles: true })); });
}

describe('the widget fit check', () => {
  it('draws each surface at its cell for the check\'s scale, on the scale\'s device', () => {
    check(['large']);
    const rem = 20;
    expect(surface('desk-tile')!.style.getPropertyValue('--widget-cell')).toBe(`${DASHBOARD_CELL_REM * rem}px`);
    expect(surface('phone-column')!.style.getPropertyValue('--widget-cell')).toBe(`${widgetCellPx(PHONE_COLUMN_PX, rem)}px`);
    expect(host.querySelector('.widget-fit-probe')!.getAttribute(WIDGET_FIT_SCALE_ATTRIBUTE)).toBe('125');
    expect(new URL(frameOn('phone-column')!.src).searchParams.get('device')).toBe('the-phone');
    expect(surface('desk-tile')!.querySelector('.widget-box')!.getAttribute('data-size')).toBe('large');
  });

  it('paints with the workspace-wide appearance when no device holds the scale', () => {
    localStorage.setItem(DEVICE_ID_KEY, 'this-window');
    try {
      check(['large'], null);
      expect(new URL(frameOn('desk-tile')!.src).searchParams.has('device')).toBe(false);
    } finally {
      localStorage.removeItem(DEVICE_ID_KEY);
    }
  });

  /** Invariant: every declared size on both surfaces. */
  it('walks every size on both surfaces, then reports each box once', async () => {
    check(['small', 'large', 'xl']);
    for (const name of ['desk-tile', 'phone-column']) {
      for (const size of ['small', 'large', 'xl']) {
        expect(surface(name)!.querySelector('.widget-box')!.getAttribute('data-size')).toBe(size);
        answer(name);
      }
    }
    await act(async () => { await Promise.resolve(); });
    expect(postWidgetFitReportApi).toHaveBeenCalledTimes(1);
    const [requestId, boxes] = postWidgetFitReportApi.mock.calls[0];
    expect(requestId).toBe('r1');
    expect(boxes.map((b) => `${b.surface} ${b.size}`)).toEqual([
      'desk-tile small', 'desk-tile large', 'desk-tile xl',
      'phone-column small', 'phone-column large', 'phone-column xl',
    ]);
    expect(widgetFitRequests.value, 'the next check may run').toEqual([]);
    expect(host.querySelector('iframe')).toBeNull();
  });

  it('carries a frame\'s failure as the box\'s error', async () => {
    check(['small']);
    answer('desk-tile', (token) => ({ token, error: 'boom' }));
    answer('phone-column');
    await act(async () => { await Promise.resolve(); });
    const boxes = postWidgetFitReportApi.mock.calls[0][1];
    expect(boxes[0]).toEqual({ surface: 'desk-tile', size: 'small', error: 'boom' });
  });

  /** Invariant: an unknown is never a pass. */
  it('tells every size a silent widget leaves, at the first deadline', async () => {
    vi.useFakeTimers();
    check(['small', 'large']);
    answer('phone-column');
    answer('phone-column');
    act(() => { vi.advanceTimersByTime(SETTLE_CAP_MS + FIT_ANSWER_SLACK_MS); });
    await act(async () => { await Promise.resolve(); });
    const boxes = postWidgetFitReportApi.mock.calls[0][1];
    expect(boxes.filter((b) => b.surface === 'desk-tile').map((b) => [b.size, typeof b.error])).toEqual([
      ['small', 'string'], ['large', 'string'],
    ]);
    expect(boxes.filter((b) => b.surface === 'phone-column').every((b) => b.error === undefined)).toBe(true);
  });

  /** The engine waits `WIDGET_FIT_BOX_BUDGET` per size. A page that may take
   *  longer than that would answer a check the engine already gave up on. */
  it('takes no longer over a box than the engine waits for one', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const rust: string = readFileSync(resolve(here, '../../../../../lucidos-engine/src/engine/widget_fit.rs'), 'utf8');
    const secs = rust.match(/WIDGET_FIT_BOX_BUDGET: Duration = Duration::from_secs\((\d+)\)/)?.[1];
    expect(secs, 'WIDGET_FIT_BOX_BUDGET is declared in engine/widget_fit.rs').toBeDefined();
    expect(SETTLE_CAP_MS + FIT_ANSWER_SLACK_MS).toBeLessThanOrEqual(Number(secs) * 1000);
  });
});
