import { writeFileSync } from 'node:fs';
import { test, expect, type Page, type Frame } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import {
  assertHealthy, blurActiveElement, disableMobileDynamicBars, disarmFollowSeed, enableMobileDynamicBars,
  ensureMobileView, gotoWithRetry, navigateToApp, sendMessage, uniqueMessage, waitForResponse,
} from './helpers';

test.use({ reducedMotion: 'no-preference' });

const APP_ID = 'e2e-dynamic-bars-motion';
const PANEL = '.mobile-swipe-pane [data-role="app-ui-panel"]';
const FRAME = `${PANEL} iframe[data-role="app-ui-frame"]`;
const THREAD = '.mobile-swipe-pane .thread-content.visible';
const SLOW = 1.5;
const BRISK = 4;

type Driver = 'screen' | 'finger';

interface Sample {
  t: number;
  segment: number;
  touching: boolean;
  headerTop: number;
  headerTarget: string;
  scrollerTop: number;
  scrollerHeight: number;
  scrollTop: number;
}

interface Metrics {
  headerFlips: number;
  headerMoves: number;
  headerMovesWhileTouching: number;
  scrollerMovesWhileTouching: number;
  rowDriftPx: number;
  scrollerResizes: number;
  longFramesInGlide: number;
  movesAfterLift: number;
  awayAfterLiftMs: number;
}

const APP_HTML = `<!DOCTYPE html>
<html><head><meta charset="UTF-8"><title>Bars motion</title>
<script src="/api/v1/sdk.js"></script>
<style>body { margin: 0; } .row { height: 50px; border-bottom: 1px solid #ccc; }</style>
</head><body><script src="script.js"></script></body></html>`;

const APP_JS = `for (let i = 0; i < 200; i++) {
  const div = document.createElement('div');
  div.className = 'row';
  div.textContent = 'Row ' + i;
  document.body.appendChild(div);
}
addEventListener('message', (e) => {
  const d = e.data;
  if (!d || typeof d !== 'object') return;
  if (d.type === 'e2e:scroll') {
    window.scrollTo(0, d.top);
    parent.postMessage({ type: 'e2e:at', top: window.scrollY }, '*');
  }
  if (d.type === 'e2e:touch') {
    const ev = new Event(d.touching ? 'touchstart' : 'touchend', { bubbles: true });
    Object.defineProperty(ev, 'touches', { value: d.touching ? [{}] : [] });
    document.body.dispatchEvent(ev);
  }
});`;

let fixture: { cleanup: () => void };

test.use({ viewport: { width: 390, height: 844 }, serviceWorkers: 'block' });

async function mirrorNotchedPhone(page: Page) {
  await page.route('**/api/v1/preferences**', async (route) => {
    if (route.request().method() !== 'GET') return route.fallback();
    const response = await route.fetch();
    const body = await response.json();
    body.preferences.backup_reminder_dismissed = 'forever';
    await route.fulfill({ response, json: body });
  });
  await page.addInitScript(() => {
    const style = document.createElement('style');
    style.textContent = ':root { --safe-area-floor-top: 47px; font-size: 18px !important; }';
    document.addEventListener('DOMContentLoaded', () => document.head.appendChild(style));
  });
}

async function trace(
  page: Page,
  opts: { target: 'app' | 'thread'; driver: Driver; speed: number; startTop: number },
): Promise<Sample[]> {
  return page.evaluate(async ({ target, driver, speed, startTop, frameSel, threadSel }) => {
    const header = document.querySelector<HTMLElement>('.app-header')!;
    const iframe = target === 'app' ? document.querySelector<HTMLIFrameElement>(frameSel)! : null;
    const thread = target === 'thread' ? document.querySelector<HTMLElement>(threadSel)! : null;
    const scroller = (iframe ?? thread)!;
    let reportedTop = startTop;
    let commandedTop = startTop;
    const onAt = (e: MessageEvent) => {
      if (e.source === iframe?.contentWindow && e.data?.type === 'e2e:at') reportedTop = e.data.top;
    };
    window.addEventListener('message', onAt);
    const scrollTo = (top: number) => {
      commandedTop = top;
      if (iframe) iframe.contentWindow!.postMessage({ type: 'e2e:scroll', top }, '*');
      else thread!.scrollTop = top;
    };
    const touch = (touching: boolean) => {
      if (iframe) iframe.contentWindow!.postMessage({ type: 'e2e:touch', touching }, '*');
    };
    const scrollTop = () => (iframe ? reportedTop : thread!.scrollTop);
    const nextFrame = () => new Promise<number>((r) => requestAnimationFrame(r));

    const DRAG = 60;
    const REST = 45;
    const segments = [
      { v: speed, frames: DRAG }, { v: 0, frames: REST },
      { v: -speed, frames: DRAG }, { v: 0, frames: REST },
    ];
    const samples: unknown[] = [];
    for (let s = 0; s < segments.length; s++) {
      const { v, frames } = segments[s];
      const touching = v !== 0;
      const segmentTop = commandedTop;
      const startScrollerTop = scroller.getBoundingClientRect().top;
      if (touching) touch(true);
      for (let k = 1; k <= frames; k++) {
        const t = await nextFrame();
        const rect = scroller.getBoundingClientRect();
        if (touching) {
          const follow = driver === 'finger' ? rect.top - startScrollerTop : 0;
          scrollTo(segmentTop + v * k + follow);
        }
        samples.push({
          t,
          segment: s,
          touching,
          headerTop: header.getBoundingClientRect().top,
          headerTarget: header.style.translate,
          scrollerTop: rect.top,
          scrollerHeight: rect.height,
          scrollTop: scrollTop(),
        });
      }
      if (touching) touch(false);
    }
    window.removeEventListener('message', onAt);
    return samples;
  }, { ...opts, frameSel: FRAME, threadSel: THREAD }) as Promise<Sample[]>;
}

function metrics(samples: Sample[]): Metrics {
  const offset = (target: string) => parseFloat(target.split(' ')[1] ?? '0') || 0;
  const segmentStart = (segment: number) => samples.find((s) => s.segment === segment)!;
  let headerFlips = 0;
  let headerMoves = 0;
  let headerMovesWhileTouching = 0;
  let scrollerMovesWhileTouching = 0;
  let rowDriftPx = 0;
  let scrollerResizes = 0;
  let longFramesInGlide = 0;
  let movesAfterLift = 0;
  const moveTimes: number[] = [];
  let lastDir = 0;
  for (let i = 1; i < samples.length; i++) {
    const prev = samples[i - 1];
    const s = samples[i];
    const start = segmentStart(s.segment);
    if (s.segment === 2 && prev.segment === 1) lastDir = 0;
    if (s.headerTarget !== prev.headerTarget) {
      headerMoves++;
      if (s.touching) headerMovesWhileTouching++;
      moveTimes.push(s.t);
      const dir = Math.sign(offset(s.headerTarget) - offset(prev.headerTarget));
      if (lastDir !== 0 && dir !== lastDir) headerFlips++;
      lastDir = dir;
      if (!s.touching && s.t - start.t > 400) movesAfterLift++;
    }
    if (s.touching && Math.abs(s.scrollerTop - prev.scrollerTop) > 1) scrollerMovesWhileTouching++;
    if (s.touching) rowDriftPx = Math.max(rowDriftPx, Math.abs(s.scrollerTop - start.scrollerTop));
    if (Math.abs(s.scrollerHeight - prev.scrollerHeight) > 0.5) scrollerResizes++;
    if (s.t - prev.t > 25 && moveTimes.some((m) => s.t >= m && s.t - m <= 400)) longFramesInGlide++;
  }
  const lift = segmentStart(1);
  const away = samples.find((s) => s.segment === 1 && offset(s.headerTarget) < 0);
  return {
    headerFlips, headerMoves, headerMovesWhileTouching, scrollerMovesWhileTouching, rowDriftPx: Math.round(rowDriftPx),
    scrollerResizes, longFramesInGlide, movesAfterLift,
    awayAfterLiftMs: away ? Math.round(away.t - lift.t) : -1,
  };
}

function report(leg: string, m: Metrics) {
  console.log(`TRACE ${leg} ${JSON.stringify(m)}`);
}

async function appAtRest(page: Page): Promise<Frame> {
  await expect(page.locator(FRAME)).toBeVisible({ timeout: 15_000 });
  const frame = (await (await page.locator(FRAME).elementHandle())!.contentFrame())!;
  await expect(frame.locator('.row').nth(199)).toBeAttached({ timeout: 10_000 });
  await frame.evaluate(() => window.scrollTo(0, 3000));
  await settle(page);
  return frame;
}

async function revealBars(page: Page) {
  await page.evaluate(() => document.dispatchEvent(new CustomEvent('reveal-mobile-bars')));
  await settle(page);
}

async function settle(page: Page) {
  let last = '';
  await expect.poll(async () => {
    const now = await page.evaluate((sel) => JSON.stringify([
      document.querySelector('.app-header')!.getBoundingClientRect().top,
      document.querySelector(sel)?.getBoundingClientRect().top ?? null,
    ]), PANEL);
    const still = now === last;
    last = now;
    return still;
  }, { timeout: 5_000, intervals: [250] }).toBe(true);
}

test.describe('dynamic bars over an app: motion trace', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Bars motion', description: 'e2e fixture' },
      html: APP_HTML,
      js: APP_JS,
    });
  });

  test.afterAll(() => fixture.cleanup());

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
    await enableMobileDynamicBars(page);
    await mirrorNotchedPhone(page);
  });

  test.afterEach(async ({ page }) => {
    await page.unrouteAll({ behavior: 'ignoreErrors' });
    await disableMobileDynamicBars(page);
  });

  for (const speed of [SLOW, BRISK]) {
    test(`the app holds still under a finger at ${speed}px a frame`, async ({ page }, info) => {
      await page.addInitScript((id) => localStorage.setItem('app-window-open', id), APP_ID);
      await gotoWithRetry(page, '/');
      await ensureMobileView(page, 'content');
      const frame = await appAtRest(page);
      const header = await page.evaluate(() => {
        const el = document.querySelector('.app-header')!;
        return {
          height: el.getBoundingClientRect().height,
          rem: getComputedStyle(document.documentElement).fontSize,
          rows: [...el.children].map((c) => `${c.className}:${Math.round(c.getBoundingClientRect().height)}`),
        };
      });
      console.log(`TRACE header ${JSON.stringify(header)}`);

      const appTop = () => frame.evaluate(() => window.scrollY);
      const finger = await trace(page, { target: 'app', driver: 'finger', speed, startTop: await appTop() });
      await revealBars(page);
      const screen = await trace(page, { target: 'app', driver: 'screen', speed, startTop: await appTop() });
      const f = metrics(finger);
      const s = metrics(screen);
      report(`app finger ${speed}`, f);
      report(`app screen ${speed}`, s);
      writeFileSync(info.outputPath(`app-trace-${speed}.json`), JSON.stringify({ finger, screen }));

      expect(f.headerFlips).toBe(0);
      expect(f.headerMovesWhileTouching).toBe(0);
      expect(f.scrollerMovesWhileTouching).toBe(0);
      expect(s.scrollerMovesWhileTouching).toBe(0);
      expect(s.rowDriftPx).toBeLessThanOrEqual(1);
      expect(f.headerMoves).toBe(2);
      expect(f.awayAfterLiftMs).toBeGreaterThanOrEqual(0);
      expect(f.movesAfterLift).toBe(0);
    });
  }

  test('a thread hides its bars during the drag, with a still scroller', async ({ page }) => {
    await disarmFollowSeed(page);
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('bars-motion')}"`);
    await waitForResponse(page);
    await blurActiveElement(page);
    await page.evaluate((sel) => {
      const container = document.querySelector(sel)!;
      const filler = document.createElement('div');
      filler.style.minHeight = '6000px';
      filler.style.flexShrink = '0';
      container.appendChild(filler);
      container.scrollTop = 2000;
    }, THREAD);
    await settle(page);

    const legs: Metrics[] = [];
    for (const speed of [SLOW, BRISK]) {
      await revealBars(page);
      const startTop = await page.evaluate((sel) => document.querySelector(sel)!.scrollTop, THREAD);
      const samples = await trace(page, { target: 'thread', driver: 'finger', speed, startTop });
      writeFileSync(test.info().outputPath(`thread-trace-${speed}.json`), JSON.stringify(samples));
      legs.push(metrics(samples));
      report(`thread finger ${speed}`, legs[legs.length - 1]);
    }
    for (const m of legs) {
      expect(m.headerFlips).toBe(0);
      expect(m.scrollerMovesWhileTouching).toBe(0);
      expect(m.headerMoves).toBe(2);
    }
  });
});
