import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  waitForThreadTitle,
  renameThreadViaMenu,
} from './helpers';
import { DASHBOARD_RAIL_SHOWN_KEY } from '../src/store/dashboardRailKeys';


function isTitleOneLine(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const el = Array.from(document.querySelectorAll('.thread-view-header .thread-title'))
      .find((e) => e.getBoundingClientRect().width > 0) as HTMLElement | undefined;
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return r.height > 0 && r.height < parseFloat(getComputedStyle(el).lineHeight) * 1.8;
  });
}

function titleTruncation(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const el = Array.from(document.querySelectorAll('.thread-view-header .thread-title-text'))
      .find((e) => e.getBoundingClientRect().width > 0) as HTMLElement;
    const header = el.closest('.thread-view-header') as HTMLElement;
    return {
      overflows: el.scrollWidth > el.clientWidth + 1,
      textOverflow: getComputedStyle(el).textOverflow,
      clipRadius: getComputedStyle(el).borderTopLeftRadius,
      visibleWidth: el.clientWidth,
      headerOverflow: header.scrollWidth - header.clientWidth,
    };
  });
}

test.describe('Thread title: desktop resize', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('title field hugs its text so the action icons sit beside it (no premature wrap)', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-hug');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const title = 'A medium length thread title';
    await renameThreadViaMenu(page, title);
    await page.waitForFunction((t) => {
      const els = document.querySelectorAll('.thread-view-header .thread-title');
      return Array.from(els).some((el) =>
        (el.textContent ?? '').trim() === t && el.getBoundingClientRect().width > 0);
    }, title, { timeout: 10_000 });

    const m = await page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.thread-view-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement;
      const display = header.querySelector('.thread-title') as HTMLElement;
      const actions = header.querySelector('.thread-view-header-actions') as HTMLElement;
      const d = display.getBoundingClientRect();
      return {
        displayWidth: d.width, displayHeight: d.height, displayRight: d.right,
        displayScrollWidth: display.scrollWidth,
        headerWidth: header.getBoundingClientRect().width,
        actionsLeft: actions.getBoundingClientRect().left,
        lineHeight: parseFloat(getComputedStyle(display).lineHeight),
      };
    });

    expect(m.displayHeight, 'title stays on one line at 1600px').toBeLessThan(m.lineHeight * 1.8);
    expect(m.displayScrollWidth, 'full title shown, not clipped')
      .toBeLessThanOrEqual(Math.ceil(m.displayWidth) + 1);
    expect(m.displayWidth, 'title field hugs its text').toBeLessThan(m.headerWidth * 0.6);
    expect(m.actionsLeft - m.displayRight, 'action icons sit just right of the title').toBeLessThan(40);

    expect((await titleTruncation(page)).overflows, 'a title that fits is not truncated at all')
      .toBe(false);
  });

  test('long title stays on one line (truncates) at narrow widths instead of wrapping', async ({ page }) => {
    await page.addInitScript((railShownKey) => {
      localStorage.setItem('lucidos-split-ratio', '1');
      localStorage.setItem(railShownKey, 'false');
    }, DASHBOARD_RAIL_SHOWN_KEY);
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-truncate');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const longTitle = 'A deliberately long thread title that will not fit a narrow desktop pane on one line, '
      + 'so the header has to truncate it with an ellipsis';
    await renameThreadViaMenu(page, longTitle);
    await page.waitForFunction((t) => {
      const els = document.querySelectorAll('.thread-view-header .thread-title');
      return Array.from(els).some((el) =>
        (el.textContent ?? '').trim() === t && el.getBoundingClientRect().width > 0);
    }, longTitle, { timeout: 10_000 });

    expect(await isTitleOneLine(page), 'one line at 1600px').toBe(true);

    await page.setViewportSize({ width: 820, height: 800 });
    await page.waitForTimeout(150);
    expect(await isTitleOneLine(page), 'still one line (truncated, not wrapped) at 820px').toBe(true);

    const narrow = await titleTruncation(page);
    expect(narrow.overflows, 'the title overflows its own box, so text-overflow applies').toBe(true);
    expect(narrow.textOverflow, 'the overflow renders as an ellipsis, not a hard clip').toBe('ellipsis');
    expect(narrow.clipRadius, 'the clip is square, so it never shaves the first glyph').toBe('0px');
    expect(narrow.headerOverflow, 'the title shrinks rather than pushing the row wider')
      .toBeLessThanOrEqual(1);

    await page.setViewportSize({ width: 1600, height: 800 });
    await page.waitForTimeout(150);
    expect(await isTitleOneLine(page), 'one line again at 1600px').toBe(true);
    expect((await titleTruncation(page)).visibleWidth, 'the title re-expands with the pane')
      .toBeGreaterThan(narrow.visibleWidth);
  });

  test('transcript top fades out under the header on desktop', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-fade');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const mask = await page.evaluate(() => {
      const el = Array.from(document.querySelectorAll('.thread-view .thread-content'))
        .find((e) => e.getBoundingClientRect().width > 0) as HTMLElement;
      const cs = getComputedStyle(el);
      const m = cs.maskImage && cs.maskImage !== 'none' ? cs.maskImage : (cs as { webkitMaskImage?: string }).webkitMaskImage;
      return m ?? 'none';
    });
    expect(mask, 'thread-content has a top fade-out mask on desktop').toContain('gradient');
  });

  test('the pin centres on the title line', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('icon-align');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const m = await page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.thread-view-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement;
      const title = header.querySelector('.thread-title') as HTMLElement;
      const t = title.getBoundingClientRect();
      const buttons = Array.from(header.querySelectorAll('.thread-view-header-actions .icon-btn')) as HTMLElement[];
      return {
        lineCentreY: t.top + parseFloat(getComputedStyle(title).lineHeight) / 2,
        buttonCentersY: buttons.map((b) => { const r = b.getBoundingClientRect(); return r.top + r.height / 2; }),
      };
    });

    expect(m.buttonCentersY.length, 'pin present').toBe(1);
    for (const cy of m.buttonCentersY) {
      expect(Math.abs(cy - m.lineCentreY), 'action icon centred on the title line').toBeLessThan(2);
    }
  });
});
