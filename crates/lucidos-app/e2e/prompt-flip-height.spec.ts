import { test, expect, type Page } from './fixtures';
import {
  navigateToApp, sendMessage, waitForResponse, uniqueMessage, assertHealthy,
  newThread, openThreadDrawer, showOngoingGroup, waitForVisibleInput, ensureOnThreadPane,
  clickVisibleElement, userMessageBody,
} from './helpers';

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

test.describe('prompt textarea height survives the compose FLIP', () => {
  test.use({ reducedMotion: 'no-preference' });

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('a multi-line draft is not clipped after sliding in from a sent thread', async ({ page }) => {
    await navigateToApp(page);

    const msg = uniqueMessage('flip-src');
    await sendMessage(page, `Say exactly: "${msg}"`);
    await waitForResponse(page);
    const activeInput = page.locator('[data-role="prompt-input"]:visible').first();
    const activeId = await activeInput.getAttribute('data-thread-id');
    expect(activeId).toBeTruthy();

    const draftText = Array.from({ length: 8 }, (_, i) => `draft line ${i + 1}`).join('\n');
    await newThread(page);
    const composeInput = await waitForVisibleInput(page);
    await composeInput.fill(draftText);
    const draftId = await composeInput.getAttribute('data-thread-id');
    expect(draftId).toBeTruthy();
    expect(draftId).not.toBe(activeId);

    const fitted = await composeInput.evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
    }));
    expect(fitted.ch, 'draft must be multi-line/tall for the test to be meaningful').toBeGreaterThan(120);
    expect(fitted.ch, 'draft starts un-clipped').toBeGreaterThanOrEqual(fitted.sh - 4);

    await openThreadDrawer(page);
    const wentToActive = await clickVisibleElement(page, `[data-thread-nav="${activeId}"]`);
    expect(wentToActive, 'sent thread row must be clickable').toBe(true);
    await ensureOnThreadPane(page);
    await expect(userMessageBody(page)).toBeVisible({ timeout: 10_000 });

    await openThreadDrawer(page);
    let wentToDraft = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    if (!wentToDraft) {
      await showOngoingGroup(page, 'Drafts');
      wentToDraft = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    }
    expect(wentToDraft, 'draft row must be clickable').toBe(true);
    await ensureOnThreadPane(page);

    const draftInput = await waitForVisibleInput(page);
    await expect(draftInput).toHaveAttribute('data-thread-id', draftId!, { timeout: 5_000 });
    await expect(draftInput).toHaveValue(draftText, { timeout: 5_000 });
    await page.waitForTimeout(600);

    const after = await draftInput.evaluate((el: HTMLTextAreaElement) => ({
      ch: el.clientHeight,
      sh: el.scrollHeight,
    }));
    expect(after.ch, 'textarea must not clip the draft after the FLIP settles').toBeGreaterThanOrEqual(after.sh - 4);
    expect(after.ch, 'textarea height must be preserved, not reset to min-height').toBeGreaterThanOrEqual(fitted.ch - 6);
  });

  test('switching between two drafts animates the textarea height', async ({ page }) => {
    await navigateToApp(page);

    const tall = Array.from({ length: 8 }, (_, i) => `A line ${i + 1}`).join('\n');
    await newThread(page);
    const aInput = await waitForVisibleInput(page);
    await aInput.fill(tall);
    const aId = await aInput.getAttribute('data-thread-id');
    expect(aId).toBeTruthy();
    const aFitted = await aInput.evaluate((el: HTMLTextAreaElement) => el.clientHeight);
    expect(aFitted, 'draft A must be tall').toBeGreaterThan(120);

    await newThread(page);
    const bInput = await waitForVisibleInput(page);
    await bInput.fill('B single line');
    const bId = await bInput.getAttribute('data-thread-id');
    expect(bId).toBeTruthy();
    expect(bId).not.toBe(aId);

    await openThreadDrawer(page);
    let wentToA = await clickVisibleElement(page, `[data-thread-nav="${aId}"]`);
    if (!wentToA) {
      await showOngoingGroup(page, 'Drafts');
      wentToA = await clickVisibleElement(page, `[data-thread-nav="${aId}"]`);
    }
    expect(wentToA, 'draft A row must be clickable').toBe(true);
    await ensureOnThreadPane(page);

    const samples = await sampleHeights(page);
    const heights = samples.map((s) => s.h);
    const minH = Math.min(...heights);
    const maxH = Math.max(...heights);
    expect(samples.some((s) => s.tr.includes('height')), 'a height transition engaged (not an insta-resize)').toBe(true);
    expect(maxH - minH, 'the height grew gradually during the animation').toBeGreaterThan(40);
    expect(samples.some((s) => s.h > minH + 15 && s.h < maxH - 15), 'intermediate heights were observed mid-animation').toBe(true);

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

  test('switching between an existing draft and the blank compose view animates', async ({ page }) => {
    await navigateToApp(page);

    const tall = Array.from({ length: 8 }, (_, i) => `blank-switch line ${i + 1}`).join('\n');
    await newThread(page);
    const draft = await waitForVisibleInput(page);
    await draft.fill(tall);
    const draftId = await draft.getAttribute('data-thread-id');
    expect(draftId).toBeTruthy();
    const fitted = await draft.evaluate((el: HTMLTextAreaElement) => el.clientHeight);
    expect(fitted, 'draft must be tall').toBeGreaterThan(120);

    await clickVisibleElement(page, 'button[aria-label="New thread"]');
    const collapse = await sampleHeights(page);
    expect(collapse.some((s) => s.tr.includes('height')), 'transition engaged: draft → blank compose').toBe(true);
    const collapseH = collapse.map((s) => s.h);
    expect(Math.max(...collapseH) - Math.min(...collapseH), 'height eased down, not snapped').toBeGreaterThan(40);

    await openThreadDrawer(page);
    let back = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    if (!back) {
      await showOngoingGroup(page, 'Drafts');
      back = await clickVisibleElement(page, `[data-thread-nav="${draftId}"]`);
    }
    expect(back, 'draft row clickable').toBe(true);
    await ensureOnThreadPane(page);
    const grow = await sampleHeights(page);
    expect(grow.some((s) => s.tr.includes('height')), 'transition engaged: blank compose → draft').toBe(true);
    const growH = grow.map((s) => s.h);
    expect(Math.max(...growH) - Math.min(...growH), 'height eased up').toBeGreaterThan(40);

    const restored = await waitForVisibleInput(page);
    await expect(restored).toHaveValue(tall, { timeout: 5_000 });
    await page.waitForTimeout(600);
    const m = await restored.evaluate((el: HTMLTextAreaElement) => ({ ch: el.clientHeight, sh: el.scrollHeight }));
    expect(m.ch, 'un-clipped after settling').toBeGreaterThanOrEqual(m.sh - 4);
  });

  test('leaving a multi-line draft for a sent thread slides from where the prompt was', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, `Say exactly: "${uniqueMessage('flip-dst')}"`);
    await waitForResponse(page);
    const activeId = await page.locator('[data-role="prompt-input"]:visible').first().getAttribute('data-thread-id');
    expect(activeId).toBeTruthy();

    await newThread(page);
    const draft = await waitForVisibleInput(page);
    await draft.fill(Array.from({ length: 8 }, (_, i) => `leaving line ${i + 1}`).join('\n'));
    expect(await draft.evaluate((el: HTMLTextAreaElement) => el.clientHeight), 'draft must be tall').toBeGreaterThan(120);
    await openThreadDrawer(page);

    const run = await page.evaluate(async (id) => {
      const prompt = () => Array.from(document.querySelectorAll<HTMLElement>('.prompt-area'))
        .find((el) => el.offsetParent !== null)!;
      const before = prompt().getBoundingClientRect().top;
      const row = Array.from(document.querySelectorAll<HTMLElement>(`[data-thread-nav="${id}"]`))
        .find((el) => el.getBoundingClientRect().height > 0);
      if (!row) return null;
      row.click();
      const tops: number[] = [];
      const start = performance.now();
      await new Promise<void>((resolve) => {
        function tick() {
          tops.push(prompt().getBoundingClientRect().top);
          if (performance.now() - start < 900) requestAnimationFrame(tick);
          else resolve();
        }
        requestAnimationFrame(tick);
      });
      return { before, tops };
    }, activeId);
    expect(run, 'sent thread row must be clickable').not.toBeNull();
    const { before, tops } = run!;
    const settled = tops[tops.length - 1];
    expect(settled - before, 'the prompt docks lower than the centered draft').toBeGreaterThan(40);
    expect(Math.abs(tops[0] - before), `first frame starts at the draft's prompt (${JSON.stringify(tops.slice(0, 4))})`).toBeLessThanOrEqual(8);
  });
});
