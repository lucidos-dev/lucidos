import { test, expect } from '@playwright/test';
import {
  assertHealthy,
  navigateToApp,
  waitForVisibleInput,
  openThreadDrawer,
} from './helpers';
import { clearAllThreads } from './db-helpers';

// Desktop-only layout test for the threads-header alternate-view toggles
// (needs-attention + drafts). The `.threads-header` (drawer header) only renders
// on desktop and depends on `page.setViewportSize()` actually changing the
// layout — which mobile-emulated projects ignore (they pin the iPhone viewport
// via `isMobile: true`). Living in a `-desktop.spec.ts` file excludes it from
// those projects (`testIgnore: /-desktop\.spec\.ts$/`).

test.describe('Threads-header alternate-view toggles — desktop layout', () => {
  test.beforeEach(async ({ page }) => {
    clearAllThreads();
    await assertHealthy(page);
  });

  test('Threads title stays put as alternate-view toggles show and hide', async ({ page }) => {
    await page.setViewportSize({ width: 1600, height: 800 });
    await navigateToApp(page);
    await openThreadDrawer(page);

    await page.waitForFunction(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0);
      const title = header?.querySelector('.threads-header-title');
      return !!title && (title as HTMLElement).getBoundingClientRect().width > 0;
    }, undefined, { timeout: 10_000 });
    // The drawer/header width animates for var(--duration-slow) (300ms).
    // Capture the no-toggle baseline after that settles so later icon
    // show/hide measurements are not mixed with the drawer-open transition.
    await page.waitForTimeout(400);

    const measure = async () => page.evaluate(() => {
      const header = Array.from(document.querySelectorAll('.threads-header'))
        .find((h) => h.getBoundingClientRect().width > 0) as HTMLElement | undefined;
      if (!header) return null;
      const slot = header.querySelector('.altview-slot') as HTMLElement | null;
      const title = header.querySelector('.threads-header-title') as HTMLElement | null;
      const filter = header.querySelector('button[aria-label="Filter threads"]') as HTMLElement | null;
      const attention = header.querySelector('button[aria-label="Toggle needs-attention view"]') as HTMLElement | null;
      const drafts = header.querySelector('button[aria-label="Toggle drafts view"]') as HTMLElement | null;
      const display = (el: HTMLElement | null) => el ? getComputedStyle(el).display : '';
      const rect = (el: HTMLElement | null) => el ? el.getBoundingClientRect() : null;
      return {
        titleTextAlign: title ? getComputedStyle(title).textAlign : '',
        titleLeft: rect(title)?.left ?? 0,
        slotLeft: rect(slot)?.left ?? 0,
        slotRight: rect(slot)?.right ?? 0,
        slotWidth: rect(slot)?.width ?? 0,
        filterRight: rect(filter)?.right ?? 0,
        draftsLeft: rect(drafts)?.left ?? 0,
        attentionWidth: rect(attention)?.width ?? 0,
        draftsWidth: rect(drafts)?.width ?? 0,
        attentionDisplay: display(attention),
        draftsDisplay: display(drafts),
        attentionDisabled: attention?.hasAttribute('disabled') ?? false,
        draftsDisabled: drafts?.hasAttribute('disabled') ?? false,
      };
    });

    const empty = await measure();
    expect(empty, 'visible threads-header with mounted altview slots').not.toBeNull();
    expect(empty!.slotWidth, 'slot reserves both desktop icon positions').toBeGreaterThan(70);
    // The slot was moved to sit between the filter icon and the Threads title.
    expect(empty!.slotLeft, 'altview slot starts right of the filter icon')
      .toBeGreaterThanOrEqual(empty!.filterRight - 1);
    expect(empty!.slotRight, 'altview slot sits left of the Threads title')
      .toBeLessThanOrEqual(empty!.titleLeft + 1);
    // The title text packs left (beside the toggles) instead of centering in the
    // gap and drifting toward the search icon.
    expect(empty!.titleTextAlign, 'Threads title text left-aligns beside the toggles')
      .toBe('left');
    // Empty toggles collapse (display:none) instead of reserving a visible box,
    // so a lone toggle can pack to the first slot right of the filter.
    expect(empty!.attentionDisplay, 'empty attention toggle collapses').toBe('none');
    expect(empty!.draftsDisplay, 'empty drafts toggle collapses').toBe('none');
    expect(empty!.attentionWidth).toBe(0);
    expect(empty!.draftsWidth).toBe(0);
    expect(empty!.attentionDisabled).toBe(true);
    expect(empty!.draftsDisabled).toBe(true);

    // Typing without sending registers an unsent draft, flipping the drafts
    // toggle visible. Needs-attention stays absent on a reset workspace, so the
    // drafts toggle is the lone occupant and packs to the first slot, while the
    // slot's reserved width keeps the title in place.
    const input = await waitForVisibleInput(page);
    await input.fill('an unsent draft to surface the drafts toggle');
    await page.waitForFunction(() => {
      const drafts = document.querySelector('.threads-header button[aria-label="Toggle drafts view"]') as HTMLElement | null;
      return !!drafts && getComputedStyle(drafts).display !== 'none';
    }, undefined, { timeout: 10_000 });

    const withDraft = await measure();
    expect(withDraft, 'threads-header after drafts toggle appears').not.toBeNull();
    expect(Math.abs(withDraft!.titleLeft - empty!.titleLeft), 'Threads title moved when drafts appeared')
      .toBeLessThan(1);
    expect(Math.abs(withDraft!.slotWidth - empty!.slotWidth), 'altview slot width changed when drafts appeared')
      .toBeLessThan(1);
    expect(withDraft!.attentionDisplay, 'absent attention toggle stays collapsed').toBe('none');
    expect(withDraft!.draftsDisplay, 'drafts toggle shows').not.toBe('none');
    expect(withDraft!.draftsWidth, 'drafts toggle has width').toBeGreaterThan(30);
    // With needs-attention absent, the drafts toggle packs to the first slot —
    // its left edge is the slot's left edge, so there is no empty gap between
    // the filter icon and the drafts toggle.
    expect(Math.abs(withDraft!.draftsLeft - withDraft!.slotLeft), 'drafts toggle takes the first slot')
      .toBeLessThan(1);
    expect(withDraft!.attentionDisabled).toBe(true);
    expect(withDraft!.draftsDisabled).toBe(false);

    await input.fill('');
    await page.waitForFunction(() => {
      const drafts = document.querySelector('.threads-header button[aria-label="Toggle drafts view"]') as HTMLElement | null;
      return !!drafts && getComputedStyle(drafts).display === 'none';
    }, undefined, { timeout: 10_000 });

    const cleared = await measure();
    expect(cleared, 'threads-header after drafts toggle hides').not.toBeNull();
    expect(Math.abs(cleared!.titleLeft - empty!.titleLeft), 'Threads title moved when drafts hid')
      .toBeLessThan(1);
    expect(Math.abs(cleared!.slotWidth - empty!.slotWidth), 'altview slot width changed when drafts hid')
      .toBeLessThan(1);
    expect(cleared!.attentionDisplay).toBe('none');
    expect(cleared!.draftsDisplay).toBe('none');
  });
});
