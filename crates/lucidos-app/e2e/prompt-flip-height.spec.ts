import { test, expect, type Page } from './fixtures';
import {
  navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy,
  newThread, openThreadDrawer, openDrawerView, waitForVisibleInput, ensureOnThreadPane,
  clickVisibleElement, isMobileViewport, userMessageBody,
} from './helpers';

/** Sample the visible prompt textarea's height + inline transition over ~900ms.
 *  The rAF loop runs in-page from the moment it's called, so it captures the
 *  full 0.3s animation window. */
function sampleHeights(page: Page): Promise<Array<{ h: number; tr: string }>> {
  return page.evaluate(async () => {
    const out: Array<{ h: number; tr: string }> = [];
    const start = performance.now();
    return await new Promise<typeof out>((resolve) => {
      function tick() {
        const el = Array.from(document.querySelectorAll<HTMLTextAreaElement>('[data-role="prompt-input"]'))
          .find((e) => e.offsetParent !== null);
        if (el) out.push({ h: el.clientHeight, tr: el.style.transition });
        if (performance.now() - start < 900) requestAnimationFrame(tick);
        else resolve(out);
      }
      tick();
    });
  });
}

/** Regression: the ThreadPane FLIP is desktop-only (skipped on mobile so iOS
 *  Safari opens the keyboard on programmatic focus), so this reproduces the
 *  reported bug only at a desktop viewport. On mobile there is no FLIP → no
 *  height wipe → nothing to assert. */
test.describe('prompt textarea height survives the compose FLIP', () => {
  // Both the ThreadPane FLIP and the draft→draft height animation are gated on
  // !prefersReducedMotion(). Headless Chromium can default prefers-reduced-motion
  // to 'reduce', which would silently skip the very animations under test (and
  // hide the regression). Force motion on so the tests actually exercise them.
  test.use({ reducedMotion: 'no-preference' });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  // Reported bug: navigating FROM a sent thread INTO a multi-line compose draft
  // ran the position-only FLIP, and clearAnimation() wiped the textarea's
  // autoResize-set height when the slide settled — so the draft snapped back to
  // min-height and clipped its own text. The height reset must only undo the
  // inline height the collapse animation itself set (animateHeight), never the
  // autoResize height owned by a position-only navigation.
  test('a multi-line draft is not clipped after sliding in from a sent thread', async ({ page }) => {
    test.skip(isMobileViewport(page), 'ThreadPane FLIP is desktop-only');

    await navigateToApp(page);

    // 1. A sent (active) thread to navigate away from.
    const msg = uniqueMessage('flip-src');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    const activeInput = page.locator('[data-role="prompt-input"]:visible').first();
    const activeId = await activeInput.getAttribute('data-thread-id');
    expect(activeId).toBeTruthy();

    // 2. A compose draft with enough lines to grow the textarea well past
    //    min-height (4.5rem ≈ 72px at the desktop root).
    const draftText = Array.from({ length: 8 }, (_, i) => `draft line ${i + 1}`).join('\n');
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await composeInput.fill(draftText);
    const draftId = await composeInput.getAttribute('data-thread-id');
    expect(draftId).toBeTruthy();
    expect(draftId).not.toBe(activeId);

    // The freshly-fitted height — the textarea shows every line, so scrollHeight
    // does not exceed clientHeight. This is the "correct" state to preserve.
    const fitted = await composeInput.evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
    }));
    expect(fitted.ch, 'draft must be multi-line/tall for the test to be meaningful').toBeGreaterThan(120);
    expect(fitted.ch, 'draft starts un-clipped').toBeGreaterThanOrEqual(fitted.sh - 4);

    // 3. Navigate to the sent thread (docked prompt), then BACK to the draft —
    //    the second hop is the sent→draft FLIP that used to wipe the height.
    await openThreadDrawer(page);
    const wentToActive = await clickVisibleElement(page, `[data-thread-nav="${activeId}"]`);
    expect(wentToActive, 'sent thread row must be clickable').toBe(true);
    await ensureOnThreadPane(page);
    await expect(userMessageBody(page)).toBeVisible({ timeout: 10_000 });

    await openThreadDrawer(page);
    // A non-focused compose-only draft shows in the drawer; fall back to the
    // dedicated Drafts view if the default listing doesn't surface it.
    let wentToDraft = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    if (!wentToDraft) {
      await openDrawerView(page, 'Drafts');
      wentToDraft = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    }
    expect(wentToDraft, 'draft row must be clickable').toBe(true);
    await ensureOnThreadPane(page);

    // 4. Draft is refocused; wait for its text to rehydrate, then let the FLIP
    //    (0.3s transition + safety timeout) fully settle.
    const draftInput = await waitForVisibleInput(page);
    await expect(draftInput).toHaveAttribute('data-thread-id', draftId!, { timeout: 5_000 });
    await expect(draftInput).toHaveValue(draftText, { timeout: 5_000 });
    await page.waitForTimeout(600);

    // 5. The regression assertion: the textarea still shows all its text (not
    //    snapped to min-height). If the bug were present, clientHeight would be
    //    ~min-height while scrollHeight stays tall → content clipped.
    const after = await draftInput.evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
    }));
    expect(after.ch, 'textarea must not clip the draft after the FLIP settles').toBeGreaterThanOrEqual(after.sh - 4);
    expect(after.ch, 'textarea height must be preserved, not reset to min-height').toBeGreaterThanOrEqual(fitted.ch - 6);
  });

  // The compose view stays centered when switching between two drafts, so the
  // ThreadPane FLIP never fires — PromptInput animates the textarea height
  // instead of insta-resizing. Assert the height transition actually engages and
  // the box lands at the new draft's full height.
  test('switching between two drafts animates the textarea height', async ({ page }) => {
    test.skip(isMobileViewport(page), 'height-FLIP is desktop-only');

    await navigateToApp(page);

    // Draft A — tall (many lines).
    const tall = Array.from({ length: 8 }, (_, i) => `A line ${i + 1}`).join('\n');
    await newThread(page);
    const aInput = await waitForVisibleInput(page);
    await aInput.fill(tall);
    const aId = await aInput.getAttribute('data-thread-id');
    expect(aId).toBeTruthy();
    const aFitted = await aInput.evaluate((el: HTMLTextAreaElement) => el.clientHeight);
    expect(aFitted, 'draft A must be tall').toBeGreaterThan(120);

    // Draft B — short (single line). Now focused.
    await newThread(page);
    const bInput = await waitForVisibleInput(page);
    await bInput.fill('B single line');
    const bId = await bInput.getAttribute('data-thread-id');
    expect(bId).toBeTruthy();
    expect(bId).not.toBe(aId);

    // Switch B → A. The height must animate: catch the transition mid-flight.
    await openThreadDrawer(page);
    let wentToA = await clickVisibleElement(page, `[data-thread-nav="${aId}"]`);
    if (!wentToA) {
      await openDrawerView(page, 'Drafts');
      wentToA = await clickVisibleElement(page, `[data-thread-nav="${aId}"]`);
    }
    expect(wentToA, 'draft A row must be clickable').toBe(true);
    await ensureOnThreadPane(page);

    // Sample the visible textarea over the next ~900ms. A `height` transition
    // must engage and the box must grow *gradually* (intermediate heights
    // between the short start and the tall target) — i.e. it animates rather
    // than insta-resizing. The rAF loop starts in-page immediately, so it
    // captures the whole 0.3s window.
    const samples = await sampleHeights(page);
    const heights = samples.map((s) => s.h);
    const minH = Math.min(...heights);
    const maxH = Math.max(...heights);
    expect(samples.some((s) => s.tr.includes('height')), 'a height transition engaged (not an insta-resize)').toBe(true);
    expect(maxH - minH, 'the height grew gradually during the animation').toBeGreaterThan(40);
    expect(samples.some((s) => s.h > minH + 15 && s.h < maxH - 15), 'intermediate heights were observed mid-animation').toBe(true);

    // After it settles, A is back at its full height and un-clipped.
    const aAgain = await waitForVisibleInput(page);
    await expect(aAgain).toHaveAttribute('data-thread-id', aId!, { timeout: 5_000 });
    await expect(aAgain).toHaveValue(tall, { timeout: 5_000 });
    await page.waitForTimeout(600);
    const metrics = await aAgain.evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
      transition: el.style.transition,
    }));
    expect(metrics.ch, 'draft A un-clipped after the height animation').toBeGreaterThanOrEqual(metrics.sh - 4);
    expect(metrics.ch, 'draft A back at its full height').toBeGreaterThanOrEqual(aFitted - 6);
    expect(metrics.transition, 'inline height transition cleared after settling').toBe('');
  });

  // The reported regression: switching between an EXISTING draft and the BLANK
  // compose view showed no animation, because the blank view has no thread id and
  // the old gate required both sides to be composing threads. The compose view
  // stays centered throughout (composeViewActive true), so the height must ease.
  test('switching between an existing draft and the blank compose view animates', async ({ page }) => {
    test.skip(isMobileViewport(page), 'height-FLIP is desktop-only');

    await navigateToApp(page);

    const tall = Array.from({ length: 8 }, (_, i) => `blank-switch line ${i + 1}`).join('\n');
    await newThread(page);
    const draft = await waitForVisibleInput(page);
    await draft.fill(tall);
    const draftId = await draft.getAttribute('data-thread-id');
    expect(draftId).toBeTruthy();
    const fitted = await draft.evaluate((el: HTMLTextAreaElement) => el.clientHeight);
    expect(fitted, 'draft must be tall').toBeGreaterThan(120);

    // Existing draft → blank compose view (New thread): the box must ease DOWN.
    await clickVisibleElement(page, 'button[aria-label="New thread"]');
    const collapse = await sampleHeights(page);
    expect(collapse.some((s) => s.tr.includes('height')), 'transition engaged: draft → blank compose').toBe(true);
    const collapseH = collapse.map((s) => s.h);
    expect(Math.max(...collapseH) - Math.min(...collapseH), 'height eased down, not snapped').toBeGreaterThan(40);

    // Blank compose view → existing draft (drawer): the box must ease UP.
    await openThreadDrawer(page);
    let back = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    if (!back) {
      await openDrawerView(page, 'Drafts');
      back = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    }
    expect(back, 'draft row clickable').toBe(true);
    await ensureOnThreadPane(page);
    const grow = await sampleHeights(page);
    expect(grow.some((s) => s.tr.includes('height')), 'transition engaged: blank compose → draft').toBe(true);
    const growH = grow.map((s) => s.h);
    expect(Math.max(...growH) - Math.min(...growH), 'height eased up').toBeGreaterThan(40);

    // Settles at the draft's full height, un-clipped.
    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue(tall, { timeout: 5_000 });
    await page.waitForTimeout(600);
    const m = await restored.evaluate((el: HTMLTextAreaElement) => ({ ch: el.clientHeight, sh: el.scrollHeight }));
    expect(m.ch, 'un-clipped after settling').toBeGreaterThanOrEqual(m.sh - 4);
  });
});
