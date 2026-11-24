import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy } from './helpers';



const LABEL = 'Said aloud on this call';

const GEOMETRY = (el: HTMLElement) => {
  el.scrollTop = el.scrollHeight;
  const turns = el.querySelectorAll('.chat-exchange');
  const last = turns[turns.length - 1] as HTMLElement | undefined;
  if (!last) return null;
  return {
    scrollHeight: el.scrollHeight,
    scrollable: el.scrollHeight > el.clientHeight + 10,
    trailing: el.scrollHeight - (last.getBoundingClientRect().bottom
      - el.getBoundingClientRect().top + el.scrollTop),
  };
};

const GROW = (el: HTMLElement) => {
  const turns = el.querySelectorAll('.chat-exchange');
  const grown = document.createElement('div');
  grown.style.height = '2000px';
  (turns[turns.length - 1] as HTMLElement).appendChild(grown);
};

const SPLICE = (el: HTMLElement, label: string) => {
  const turns = el.querySelectorAll('.chat-exchange');
  const span = document.createElement('span');
  span.className = 'visually-hidden';
  span.textContent = label;
  (turns[turns.length - 1] as HTMLElement).appendChild(span);
};

test.describe('A hidden label adds no scrollable overflow', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('splicing a hidden label moves nothing on a scrolling transcript', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, 'Reply with exactly the word: one');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const geometry = () => tc.evaluate(GEOMETRY);

    await expect.poll(async () => (await geometry()) !== null,
      { message: 'no turn rendered to measure' }).toBe(true);
    await tc.evaluate(GROW);

    const before = (await geometry())!;
    expect(before.scrollable, 'the transcript never overflowed its pane').toBe(true);

    await tc.evaluate(SPLICE, LABEL);

    const after = (await geometry())!;
    expect(
      after.scrollHeight - before.scrollHeight,
      `the hidden label added ${after.scrollHeight - before.scrollHeight}px of scrollable height`,
    ).toBeLessThanOrEqual(1);
    expect(
      after.trailing - before.trailing,
      `the hidden label opened ${Math.round(after.trailing - before.trailing)}px under the last turn`,
    ).toBeLessThanOrEqual(1);
  });

  test('and the same splice does open a hole once the clip is taken away', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, 'Reply with exactly the word: two');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const geometry = () => tc.evaluate(GEOMETRY);
    await expect.poll(async () => (await geometry()) !== null,
      { message: 'no turn rendered to measure' }).toBe(true);
    await tc.evaluate(GROW);

    await page.addStyleTag({
      content: '.visually-hidden { overflow: visible !important; white-space: normal !important; }',
    });
    const before = (await geometry())!;

    await tc.evaluate(SPLICE, LABEL);

    const after = (await geometry())!;
    expect(
      after.scrollHeight - before.scrollHeight,
      'the unclipped label added no scroll, so the check above proves nothing',
    ).toBeGreaterThan(20);
  });
});
