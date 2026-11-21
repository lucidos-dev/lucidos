/**
 * The Lucidos tooltip inside an app.
 *
 * The contract an app author gets is two things: load `sdk.js`, write
 * `data-tooltip`. Every fixture below sticks to that. None of their scripts
 * calls an SDK function, which is what "zero config" has to mean.
 *
 * One implementation serves the host shell and every app. The behaviour is
 * `installTooltips()` in `packages/lucidos-sdk/src/tooltip.ts` and the CSS is in
 * `styles/global/shared-components.css`, which the engine appends to the served
 * `/api/v1/sdk-iframe.css`.
 */
import { test, expect, Page } from './fixtures';
import { appPath, createIframeAppFixture } from './db-helpers';
import { gotoWithRetry, isMobileViewport } from './helpers';

/** Long enough to clear the layer's 450ms long-press threshold. */
const LONG_PRESS_WAIT_MS = 700;

/** The SDK asks for a 2000ms auto-clear after the finger lifts. Wait past it. */
const AUTO_CLEAR_WAIT_MS = 4000;

/**
 * Dispatch one synthetic touch event on `selector`, offset `dx` px from the
 * element's centre.
 *
 * Plain `Event`s with hand-defined touch lists, not `new TouchEvent()`: that
 * constructor is illegal in WebKit, one of the three projects this spec runs
 * in. The tooltip reads only `touches[0]` and `changedTouches[0]`, which the
 * defined properties supply.
 *
 * Touch is the reveal used by most cases here, because it works on all three
 * projects. The mouse path is deliberately inert on a touch device, so a
 * hover-driven case would pass vacuously on the two mobile projects.
 */
async function touch(
  page: Page,
  selector: string,
  type: 'touchstart' | 'touchmove' | 'touchend',
  dx = 0,
): Promise<void> {
  await page.evaluate(({ sel, kind, offset }) => {
    const el = document.querySelector<HTMLElement>(sel);
    if (!el) throw new Error(`no element for ${sel}`);
    const r = el.getBoundingClientRect();
    const x = r.left + r.width / 2 + offset;
    const y = r.top + r.height / 2;
    const ev = new Event(kind, { bubbles: true, cancelable: true, composed: true });
    const point = { identifier: 1, target: el, clientX: x, clientY: y, pageX: x, pageY: y };
    const list = kind === 'touchend' ? [] : [point];
    Object.defineProperty(ev, 'touches', { value: list });
    Object.defineProperty(ev, 'targetTouches', { value: list });
    Object.defineProperty(ev, 'changedTouches', { value: [point] });
    el.dispatchEvent(ev);
  }, { sel: selector, kind: type, offset: dx });
}

/** Press and hold `selector` until the tooltip has had time to appear. */
async function longPress(page: Page, selector: string): Promise<void> {
  await touch(page, selector, 'touchstart');
  await page.waitForTimeout(LONG_PRESS_WAIT_MS);
}

/**
 * Long-press `selector`, lift, then fire the click a browser sends after that
 * gesture. Answers true when the click reached the element.
 *
 * The synthetic touch events above produce no click of their own, so the test
 * sends it. The layer swallows it at the document capture phase.
 */
async function longPressThenClick(page: Page, selector: string): Promise<boolean> {
  await page.evaluate((sel) => {
    const el = document.querySelector<HTMLElement>(sel);
    if (!el) throw new Error(`no element for ${sel}`);
    const w = window as unknown as { __clicked: boolean };
    w.__clicked = false;
    el.addEventListener('click', () => { w.__clicked = true; });
  }, selector);

  await longPress(page, selector);
  await touch(page, selector, 'touchend');
  await page.evaluate((sel) => document.querySelector<HTMLElement>(sel)?.click(), selector);
  return page.evaluate(() => (window as unknown as { __clicked: boolean }).__clicked);
}

/** How many `#tooltip` nodes the document holds, and who owns them. */
async function tooltipOwners(page: Page): Promise<(string | null)[]> {
  return page.evaluate(() =>
    Array.from(document.querySelectorAll('#tooltip')).map((el) => el.getAttribute('data-owner')));
}

/** Geometry of the live tooltip against the element it points at. */
async function measure(page: Page, selector: string) {
  return page.evaluate((sel) => {
    const tip = document.querySelector<HTMLElement>('#tooltip');
    const arrow = document.querySelector<HTMLElement>('#tooltip-arrow');
    const el = document.querySelector<HTMLElement>(sel);
    if (!tip || !arrow || !el) throw new Error('tooltip or target missing');
    const t = tip.getBoundingClientRect();
    const a = arrow.getBoundingClientRect();
    const e = el.getBoundingClientRect();
    return {
      above: tip.classList.contains('above'),
      tipLeft: t.left,
      tipRight: t.right,
      tipTop: t.top,
      tipBottom: t.bottom,
      arrowCentre: a.left + a.width / 2,
      targetCentre: e.left + e.width / 2,
      targetTop: e.top,
      targetBottom: e.bottom,
      viewportWidth: window.innerWidth,
    };
  }, selector);
}

// Fixed pixel positions, not rem: the assertions below are geometric, and a rem
// would move with whatever root font size the served stylesheet resolves to.
const PROBE_STYLE = `
  body { margin: 0; }
  .probe { position: fixed; width: 4rem; height: 2rem; }
  #near-top { top: 0; left: 140px; }
  #mid { top: 300px; left: 140px; }
  #left-edge { top: 380px; left: 0; width: 20px; }
  #right-edge { top: 380px; right: 0; width: 20px; }
`;

const APP_ID = 'e2e-sdk-tooltip-test';
let fixture: { cleanup: () => void };

test.describe('SDK iframe tooltip', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK tooltip test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
<style>${PROBE_STYLE}</style>
</head>
<body>
<button class="probe" id="near-top" data-tooltip="No room above this one">Top</button>
<button class="probe" id="mid" data-tooltip="Hello from the SDK" data-tooltip-title="Shared tooltip">Middle</button>
<button class="probe" id="left-edge" data-tooltip="Stays inside the viewport">L</button>
<button class="probe" id="right-edge" data-tooltip="Stays inside the viewport">R</button>
<div id="status">init</div>
<script src="script.js"></script>
</body>
</html>
`,
      // The whole app script. It marks the page ready and does nothing else. No
      // init call, no SDK reference: `sdk.js` installed the layer already.
      js: `document.querySelector('#status').textContent = 'ready';`,
    });
  });

  test.afterAll(() => {
    fixture.cleanup();
  });

  test('a hover reveals the tooltip above the element, and mouseout hides it', async ({ page }) => {
    test.skip(isMobileViewport(page), 'Pointer hover only. The touch cases below cover mobile.');
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    // Time the reveal from inside the page, so Playwright's own action latency
    // cannot inflate the number and hide a missing delay.
    await page.evaluate(() => {
      (window as unknown as { __tipDelay: Promise<number> }).__tipDelay = new Promise<number>((resolve) => {
        document.addEventListener('mouseover', function first() {
          document.removeEventListener('mouseover', first, true);
          const t0 = performance.now();
          const poll = setInterval(() => {
            const tip = document.querySelector<HTMLElement>('#tooltip');
            if (tip && tip.style.opacity === '1') {
              clearInterval(poll);
              resolve(performance.now() - t0);
            }
          }, 10);
          setTimeout(() => { clearInterval(poll); resolve(-1); }, 5000);
        }, true);
      });
    });

    await page.hover('#mid');
    const tooltip = page.locator('#tooltip');
    await expect(tooltip).toBeVisible({ timeout: 5000 });

    const delayMs = await page.evaluate(() => (window as unknown as { __tipDelay: Promise<number> }).__tipDelay);
    expect(delayMs, 'the layer waits out its hover delay before showing').toBeGreaterThan(200);

    await expect(page.locator('#tooltip-title')).toHaveText('Shared tooltip');
    await expect(page.locator('#tooltip-text')).toHaveText('Hello from the SDK');

    const geom = await measure(page, '#mid');
    expect(geom.above, 'above by default, when there is room').toBe(true);
    expect(geom.tipBottom).toBeLessThanOrEqual(geom.targetTop);
    // Within the 1px border, the arrow sits on the element's centre.
    expect(Math.abs(geom.arrowCentre - geom.targetCentre)).toBeLessThan(2);

    await page.mouse.move(5, 5);
    await expect(tooltip).toBeHidden();
  });

  test('with no room above, the tooltip flips below', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    await longPress(page, '#near-top');
    await expect(page.locator('#tooltip')).toBeVisible();

    const geom = await measure(page, '#near-top');
    expect(geom.above, 'the flip drops the .above class, which flips the arrow').toBe(false);
    expect(geom.tipTop).toBeGreaterThanOrEqual(geom.targetBottom);
  });

  test('a tooltip wider than its anchor is clamped inside the viewport', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    await longPress(page, '#left-edge');
    await expect(page.locator('#tooltip')).toBeVisible();
    const left = await measure(page, '#left-edge');
    expect(left.tipLeft, 'centring on a left-edge anchor would run off screen').toBeLessThan(9.5);
    expect(left.tipLeft).toBeGreaterThan(6.5);
    expect(left.arrowCentre).toBeGreaterThan(left.tipLeft);
    expect(left.arrowCentre).toBeLessThan(left.tipRight);

    await touch(page, '#left-edge', 'touchend');
    await longPress(page, '#right-edge');
    const right = await measure(page, '#right-edge');
    expect(Math.abs(right.tipRight - (right.viewportWidth - 8))).toBeLessThan(1.5);
    expect(right.arrowCentre).toBeGreaterThan(right.tipLeft);
    expect(right.arrowCentre).toBeLessThan(right.tipRight);
  });

  test('an element added after load is covered too', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    // One delegated document listener, so nothing rescans and the app wires
    // nothing per element.
    await page.evaluate(() => {
      const btn = document.createElement('button');
      btn.id = 'added-later';
      btn.setAttribute('data-tooltip', 'Added after the page loaded');
      btn.textContent = 'Later';
      btn.style.cssText = 'position:fixed;top:200px;left:140px;width:64px;height:32px;';
      document.body.appendChild(btn);
    });

    await longPress(page, '#added-later');
    await expect(page.locator('#tooltip')).toBeVisible();
    await expect(page.locator('#tooltip-text')).toHaveText('Added after the page loaded');
  });

  test('a long press reveals, and the tooltip clears itself after the finger lifts', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    const tooltip = page.locator('#tooltip');
    await longPress(page, '#mid');
    await expect(tooltip).toBeVisible();

    await touch(page, '#mid', 'touchend');
    await expect(tooltip, 'the release keeps it up briefly, so the user can read it').toBeVisible();
    await expect(tooltip).toBeHidden({ timeout: AUTO_CLEAR_WAIT_MS });
  });

  test('moving the finger cancels the pending long press', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    // A scroll or a swipe must never trail a tooltip behind it.
    await touch(page, '#mid', 'touchstart');
    await touch(page, '#mid', 'touchmove', 60);
    await page.waitForTimeout(LONG_PRESS_WAIT_MS);
    await touch(page, '#mid', 'touchend');

    await expect(page.locator('#tooltip')).toBeHidden();
  });

  test('the tap that ends a long press does not also activate the element', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    expect(
      await longPressThenClick(page, '#mid'),
      'the reveal claimed the gesture, so its terminating tap is swallowed',
    ).toBe(false);
  });

  test('lucidos.ui.disableTooltips() stands the layer down', async ({ page }) => {
    await gotoWithRetry(page, appPath(APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    await page.evaluate(() => (window as unknown as {
      lucidos: { ui: { disableTooltips: () => void } };
    }).lucidos.ui.disableTooltips());
    await expect(page.locator('html')).toHaveAttribute('data-lucidos-tooltips', 'off');

    await longPress(page, '#mid');
    expect(await tooltipOwners(page), 'the opted-out app builds no tooltip node at all').toEqual([]);
  });
});

const COLLISION_APP_ID = 'e2e-sdk-tooltip-collision';
let collisionFixture: { cleanup: () => void };

// An app that hand-rolled a tooltip before the SDK shipped one. Two tooltips on
// one screen is worse than none, so the layer stands down for it, unprompted.
test.describe('SDK iframe tooltip: the app owns a #tooltip', () => {
  test.beforeAll(() => {
    collisionFixture = createIframeAppFixture(COLLISION_APP_ID, {
      html: `<!DOCTYPE html>
<html>
<head>
<meta charset="UTF-8">
<title>SDK tooltip collision test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
<style>${PROBE_STYLE}</style>
</head>
<body>
<button class="probe" id="mid" data-tooltip="Hello from the SDK">Middle</button>
<div id="status">init</div>
<script src="script.js"></script>
</body>
</html>
`,
      // The node is built at the end of <body>, which runs after sdk.js in
      // <head>. So an install-time collision check would look too early to see
      // it, and only a check before every show catches this.
      js: `
var own = document.createElement('div');
own.id = 'tooltip';
own.setAttribute('data-owner', 'app');
own.textContent = 'the app tooltip';
document.body.appendChild(own);
document.querySelector('#status').textContent = 'ready';
`,
    });
  });

  test.afterAll(() => {
    collisionFixture.cleanup();
  });

  test('the SDK stands down rather than showing a second tooltip', async ({ page }) => {
    await gotoWithRetry(page, appPath(COLLISION_APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    await longPress(page, '#mid');
    expect(await tooltipOwners(page), 'the app keeps its own node, and gains no second one').toEqual(['app']);
  });

  test('a long press still activates the element, since nothing was revealed', async ({ page }) => {
    await gotoWithRetry(page, appPath(COLLISION_APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    expect(
      await longPressThenClick(page, '#mid'),
      'a stood-down layer shows nothing, so it must not swallow the click either',
    ).toBe(true);
  });
});

const OPT_OUT_APP_ID = 'e2e-sdk-tooltip-opt-out';
let optOutFixture: { cleanup: () => void };

test.describe('SDK iframe tooltip: markup opt-out', () => {
  test.beforeAll(() => {
    optOutFixture = createIframeAppFixture(OPT_OUT_APP_ID, {
      // The attribute applies before any script runs, so an app can decline the
      // layer without loading anything to say so.
      html: `<!DOCTYPE html>
<html data-lucidos-tooltips="off">
<head>
<meta charset="UTF-8">
<title>SDK tooltip opt-out test</title>
<link rel="stylesheet" href="/api/v1/sdk-iframe.css">
<script src="/api/v1/sdk.js"></script>
<style>${PROBE_STYLE}</style>
</head>
<body>
<button class="probe" id="mid" data-tooltip="Hello from the SDK">Middle</button>
<div id="status">init</div>
<script src="script.js"></script>
</body>
</html>
`,
      js: `document.querySelector('#status').textContent = 'ready';`,
    });
  });

  test.afterAll(() => {
    optOutFixture.cleanup();
  });

  test('data-lucidos-tooltips="off" on <html> turns the layer off', async ({ page }) => {
    await gotoWithRetry(page, appPath(OPT_OUT_APP_ID));
    await expect(page.locator('#status')).toHaveText('ready');

    await longPress(page, '#mid');
    expect(await tooltipOwners(page)).toEqual([]);
  });
});
