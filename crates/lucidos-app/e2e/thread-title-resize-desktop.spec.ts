import { test, expect } from './fixtures';
import {
  assertHealthy,
  navigateToApp,
  sendMessage,
  waitForResponse,
  uniqueMessage,
  clickVisibleElement,
  waitForThreadTitle,
  waitForTitleInput,
} from './helpers';

// Desktop-only thread-title layout tests — they depend on `page.setViewportSize()`
// actually changing the rendered layout, which mobile-emulated projects (mobile,
// mobile-webkit) ignore because they pin the iPhone viewport via `isMobile: true`.
// Living in a `-desktop.spec.ts` file excludes them from those projects
// (`testIgnore: /-desktop\.spec\.ts$/`). The edit tests that pass under mobile
// emulation stay in thread-title-edit.spec.ts.

/** True if the visible desktop title display is rendered on a single line. */
function isTitleOneLine(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const el = Array.from(document.querySelectorAll('.thread-view-header .thread-title-display'))
      .find((e) => e.getBoundingClientRect().width > 0) as HTMLElement | undefined;
    if (!el) return false;
    const r = el.getBoundingClientRect();
    return r.height > 0 && r.height < parseFloat(getComputedStyle(el).lineHeight) * 1.8;
  });
}

/** How the visible desktop title display truncates: whether its text overflows
 *  its OWN box (the precondition for text-overflow to fire at all) and what
 *  `text-overflow` computes to. A hard clip reads as `overflows: false` no
 *  matter how much title is missing, because the box was sized to the text and
 *  an ancestor did the cutting. */
function titleTruncation(page: import('@playwright/test').Page) {
  return page.evaluate(() => {
    const el = Array.from(document.querySelectorAll('.thread-view-header .thread-title-display'))
      .find((e) => e.getBoundingClientRect().width > 0) as HTMLElement;
    const header = el.closest('.thread-view-header') as HTMLElement;
    return {
      overflows: el.scrollWidth > el.clientWidth + 1,
      textOverflow: getComputedStyle(el).textOverflow,
      visibleWidth: el.clientWidth,
      // The row itself must never overflow: the title shrinks, it doesn't push.
      headerOverflow: header.scrollWidth - header.clientWidth,
    };
  });
}

test.describe('Thread title editing — desktop resize', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('title field hugs its text so the action icons sit beside it (no premature wrap)', async ({ page }) => {
    // The display <div> uses white-space:nowrap + align-self:stretch against a
    // content-sized wrapper, so it hugs the title on one line and the copy/export
    // icons sit right beside it, not pushed to the row's far edge and not wrapped
    // early. Guards the regression where a <textarea>/intrinsic sizing collapsed
    // the field and wrapped with room to spare.
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-hug');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const title = 'A medium length thread title';
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);
    await input.fill(title);
    await input.press('Enter');
    await page.waitForFunction((t) => {
      const els = document.querySelectorAll('.thread-view-header .thread-title-display');
      return Array.from(els).some((el) =>
        (el.textContent ?? '').trim() === t && el.getBoundingClientRect().width > 0);
    }, title, { timeout: 10_000 });

    const m = await page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.thread-view-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement;
      const display = header.querySelector('.thread-title-display') as HTMLElement;
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

    // One line, not wrapped.
    expect(m.displayHeight, 'title stays on one line at 1600px').toBeLessThan(m.lineHeight * 1.8);
    // Full title shown — not clipped (no horizontal overflow of its own box).
    expect(m.displayScrollWidth, 'full title shown, not clipped')
      .toBeLessThanOrEqual(Math.ceil(m.displayWidth) + 1);
    // Hugs its text — far narrower than the header (doesn't fill the row).
    expect(m.displayWidth, 'title field hugs its text').toBeLessThan(m.headerWidth * 0.6);
    // Icons sit right beside the title, not at the row's far edge.
    expect(m.actionsLeft - m.displayRight, 'action icons sit just right of the title').toBeLessThan(40);

    // No ellipsis on a title that fits. The leaf's negative horizontal margins
    // make a naive `max-width: 100%` land 0.5rem short of the text, which
    // ellipsises EVERY title ~1 char early; `align-self: stretch` doesn't.
    expect((await titleTruncation(page)).overflows, 'a title that fits is not truncated at all')
      .toBe(false);
  });

  test('long title stays on one line (truncates) at narrow widths instead of wrapping', async ({ page }) => {
    // The title field is always one line: it hugs a short title and clips a
    // too-long one at the row's edge, rather than wrapping the header to 2+ lines.
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);

    const msg = uniqueMessage('title-truncate');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    const longTitle = 'A deliberately long thread title that will not fit a narrow desktop pane on one line';
    await clickVisibleElement(page, '.thread-title-display');
    const input = await waitForTitleInput(page);
    await input.fill(longTitle);
    await input.press('Enter');
    await page.waitForFunction((t) => {
      const els = document.querySelectorAll('.thread-view-header .thread-title-display');
      return Array.from(els).some((el) =>
        (el.textContent ?? '').trim() === t && el.getBoundingClientRect().width > 0);
    }, longTitle, { timeout: 10_000 });

    expect(await isTitleOneLine(page), 'one line at 1600px').toBe(true);

    // Narrow the pane (stay >768px to keep the desktop layout). The long title no
    // longer fits — it must truncate to one line, not wrap the header taller.
    await page.setViewportSize({ width: 820, height: 800 });
    await page.waitForTimeout(150);
    expect(await isTitleOneLine(page), 'still one line (truncated, not wrapped) at 820px').toBe(true);

    // Regression: the overflowing title was hard-cut mid-word with no ellipsis.
    // The display leaf was `width: max-content`, so its text never overflowed
    // its OWN box (text-overflow can only fire on self-overflow) and the wrapper
    // clipped it with a bare `overflow: hidden`. Both halves are asserted: the
    // text must overflow the leaf, AND the leaf must be the one truncating.
    const narrow = await titleTruncation(page);
    expect(narrow.overflows, 'the title overflows its own box, so text-overflow applies').toBe(true);
    expect(narrow.textOverflow, 'the overflow renders as an ellipsis, not a hard clip').toBe('ellipsis');
    expect(narrow.headerOverflow, 'the title shrinks rather than pushing the row wider')
      .toBeLessThanOrEqual(1);

    // Widen again: still one line, and more of the title is visible. Asserted as
    // a width recovery rather than "no longer truncated" on purpose: whether an
    // 84-char title fits the thread pane at a 1600px viewport depends on the
    // split ratio and the font, so a no-ellipsis assertion here would be a
    // coin-flip. The fits-with-no-ellipsis case is covered by the medium-title
    // test above, which pins the title at under 60% of the row.
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

  test('copy/export icons stay centered on the input field while editing (suggestion below)', async ({ page }) => {
    // Regression: the title editor lays the input and its suggestion out as a
    // flex column. While editing, the suggestion row sits below the input and
    // made the column taller — and the header's align-items:center then dropped
    // the copy/export icons to the middle of the whole block instead of keeping
    // them centered on the input field. The icons must track the input row, with
    // the suggestion hanging below the whole row.
    await page.setViewportSize({ width: 1600, height: 800 });

    // Force a stable suggestion so the suggestion row renders below the input
    // (the element that inflated the column and caused the misalignment).
    await page.route('**/api/v1/threads/suggest-title', async (route) => {
      await route.fulfill({
        status: 200,
        contentType: 'application/json',
        body: JSON.stringify({ title: 'A Suggested Thread Title' }),
      });
    });

    await navigateToApp(page);

    const msg = uniqueMessage('icon-align');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    await waitForThreadTitle(page);

    await clickVisibleElement(page, '.thread-title-display');
    await waitForTitleInput(page);

    // Wait until the suggestion row is actually rendered with height.
    await page.waitForFunction(() => {
      const el = Array.from(document.querySelectorAll('.thread-view-header .thread-title-suggestion'))
        .find((e) => e.getBoundingClientRect().width > 0);
      return !!el && el.getBoundingClientRect().height > 0;
    }, undefined, { timeout: 10_000 });

    const m = await page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.thread-view-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement;
      const input = header.querySelector('input.thread-title-edit-input') as HTMLElement;
      const suggestion = header.querySelector('.thread-title-suggestion') as HTMLElement;
      const buttons = Array.from(header.querySelectorAll('.thread-view-header-actions .icon-btn')) as HTMLElement[];
      const ir = input.getBoundingClientRect();
      const sr = suggestion.getBoundingClientRect();
      return {
        inputCenterY: ir.top + ir.height / 2,
        inputBottom: ir.bottom,
        suggestionTop: sr.top,
        suggestionHeight: sr.height,
        buttonCentersY: buttons.map((b) => { const r = b.getBoundingClientRect(); return r.top + r.height / 2; }),
        buttonCount: buttons.length,
      };
    });

    // Preconditions: both action icons present and the suggestion sits below the input.
    expect(m.buttonCount, 'copy + export icons present').toBe(2);
    expect(m.suggestionHeight, 'suggestion row is rendered').toBeGreaterThan(0);
    expect(m.suggestionTop, 'suggestion hangs below the input row').toBeGreaterThanOrEqual(m.inputBottom - 2);

    // The fix: each icon's vertical center tracks the input field's center
    // (not the center of input+suggestion, which was ~12px lower).
    for (const cy of m.buttonCentersY) {
      expect(Math.abs(cy - m.inputCenterY), 'action icon centered on the input field')
        .toBeLessThan(4);
    }
  });
});
