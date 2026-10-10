import { test, expect } from './fixtures';
import { navigateToApp, sendMessage, waitForResponse, assertHealthy } from './helpers';

/** A visually hidden label may not scroll the transcript it sits in.
 *
 *  `.visually-hidden` keeps its element IN layout on purpose, and a zero-width
 *  box still lays its text out. With no room the line breaks after every
 *  character, and that column is scrollable overflow no element's rect shows.
 *  A call is where it was reported: `SpokenReply` draws one label per reply.
 *  Cause, numbers and what was ruled out:
 *  docs/plans/2026-09-17-a-hidden-label-stops-inflating-the-transcript.md
 *
 *  Only a real engine lays out the text the fix clips, so this cannot be a unit
 *  test. `styles/__tests__/a-hidden-label-clips-its-text.test.ts` pins the two
 *  declarations; this measures what they prevent. */

/* Three things make the measurement mean what it says.
 *
 * It measures a DELTA. How much space a transcript leaves under its last turn
 * is ADR 0212's decision, and `transcript-ends-where-its-content-ends.spec.ts`
 * owns it. This owns only that a hidden label changes neither number.
 *
 * It splices the LABEL alone, never the `.spoken-reply-who` wrapper. That
 * wrapper is an `inline-flex` box worth a 16px line box. It is a real box
 * doing a real job, and it is not the property under test.
 *
 * It grows the transcript past its pane first, because the report is a thread
 * that SCROLLS. Splicing beats placing a call, which needs a microphone and a
 * provider, and the property belongs to the label rather than to the call.
 */

/** Long enough that an unclipped box stacks a visible column of characters
 *  under the newest turn. `SpokenReply`'s own label is ten. */
const LABEL = 'Said aloud on this call';

/** Put the reader on the live edge and read the two numbers the bug moved. */
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

/** Push the transcript past its pane. A call gets there by talking; one block
 *  is the same shape and takes no minutes. */
const GROW = (el: HTMLElement) => {
  const turns = el.querySelectorAll('.chat-exchange');
  const grown = document.createElement('div');
  grown.style.height = '2000px';
  (turns[turns.length - 1] as HTMLElement).appendChild(grown);
};

/** `SpokenReply`'s own label, on the newest turn. */
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
    // Within a pixel, because `scrollHeight` is an integer and the rect
    // differenced against it is fractional.
    expect(
      after.scrollHeight - before.scrollHeight,
      `the hidden label added ${after.scrollHeight - before.scrollHeight}px of scrollable height`,
    ).toBeLessThanOrEqual(1);
    expect(
      after.trailing - before.trailing,
      `the hidden label opened ${Math.round(after.trailing - before.trailing)}px under the last turn`,
    ).toBeLessThanOrEqual(1);
  });

  /* THE CANARY. The check above passes trivially if the splice never reaches
   * layout. So this one puts the old rule back and proves the same splice is
   * measurable without the clip. A typo in the class name would otherwise read
   * as a fix forever.
   *
   * The old rule goes on BEFORE the baseline is taken, so the delta isolates
   * the splice. Injected after, any other hidden label on the page would
   * un-clip inside the measurement and could satisfy the assertion alone.
   */
  test('and the same splice does open a hole once the clip is taken away', async ({ page }) => {
    await navigateToApp(page);
    await sendMessage(page, 'Reply with exactly the word: two');
    await waitForResponse(page);

    const tc = page.locator('.thread-content.visible:visible').first();
    const geometry = () => tc.evaluate(GEOMETRY);
    await expect.poll(async () => (await geometry()) !== null,
      { message: 'no turn rendered to measure' }).toBe(true);
    await tc.evaluate(GROW);

    // The rule as it shipped before the fix: hidden, in layout, unclipped.
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
