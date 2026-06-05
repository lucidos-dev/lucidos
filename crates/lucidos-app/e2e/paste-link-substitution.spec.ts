import { test, expect, Locator } from '@playwright/test';
import { navigateToApp, waitForVisibleInput, assertHealthy } from './helpers';

/** Dispatch a real paste event with the given clipboard text, mimicking the
 *  browser's clipboard delivery. Bypasses permission prompts that block real
 *  clipboard reads in Playwright.
 *
 *  `selection` (optional [start, end]) is re-applied in the SAME evaluate as the
 *  dispatch. Setting the selection in an earlier evaluate races on WebKit: the
 *  prior value/input re-render can land between the two round-trips and collapse
 *  the selection, so the handler sees no selection and falls through (no
 *  substitution). Applying it atomically with the dispatch closes that window. */
async function pasteText(input: Locator, text: string, selection?: [number, number]) {
  // Let the app's async draft-settle finish before issuing the synthetic paste.
  // Setting `el.value` + dispatching `input` promotes a compose thread; that
  // schedules a textarea re-sync effect carrying the PRE-paste draft snapshot.
  // If that effect runs AFTER the paste's setRangeText (cold start / load delays
  // it past our next evaluate), it reverts the substitution back to the stale
  // draft value. Waiting two animation frames guarantees the promotion effect
  // has run, so the paste mutation is the last write to the textarea. A real
  // user can't type-then-paste inside one frame, so this only closes a synthetic
  // -input race, not a product behavior gap.
  await input.evaluate(
    () => new Promise<void>((r) => requestAnimationFrame(() => requestAnimationFrame(() => r()))),
  );
  await input.evaluate(
    (el: HTMLTextAreaElement, { payload, sel }: { payload: string; sel?: [number, number] }) => {
      el.focus();
      if (sel) el.setSelectionRange(sel[0], sel[1]);
      const dt = new DataTransfer();
      dt.setData('text/plain', payload);
      el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    },
    { payload: text, sel: selection },
  );
}

test.describe('Paste link substitution (Slack-style)', () => {
  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test('wraps selection as title when pasting a thread ref onto a selection', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    const ref = 'thread:dev/11111111-2222-3333-4444-555555555555';
    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = 'See yesterday for context.';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    });

    await pasteText(input, ref, [4, 13]); // select "yesterday"

    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe(`See [yesterday](${ref}) for context.`);
  });

  test('wraps selection as title when pasting a markdown link onto a selection', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = 'Read the article first.';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    });

    await pasteText(input, '[Original Title](https://vg.no/news/123)', [9, 16]); // select "article"

    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe('Read the [article](https://vg.no/news/123) first.');
  });

  test('falls through to default paste when no selection', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = '';
    });

    // No selection → handler must not preventDefault → textarea stays empty
    // (real paste fills it; our synthesized event has no native default).
    await pasteText(input, 'https://vg.no/article', [0, 0]);
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe('');
  });

  test('falls through to default paste for non-URL text on selection', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = 'hello world';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    });

    await pasteText(input, 'localhost:3000', [6, 11]); // select "world"
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    // Handler should not intercept; selection stays as-is (no default-paste
    // happens for synthesized events, so value is unchanged).
    expect(value).toBe('hello world');
  });

  test('escapes ] in selection so the link title cannot close early', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = 'see array[0] now';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    });

    await pasteText(input, 'https://vg.no/x', [4, 12]); // select "array[0]"
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe('see [array[0\\]](https://vg.no/x) now');
  });

  test('falls through when clipboard URL contains markdown delimiters (injection guard)', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = 'click here please';
      el.dispatchEvent(new Event('input', { bubbles: true }));
    });

    // Crafted payload: if the handler accepted this verbatim it would emit
    // [here](https://evil/)[real](https://good) — two links, with "here"
    // pointing at the attacker's URL.
    await pasteText(input, 'https://evil/)[real](https://good', [6, 10]); // select "here"
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    // No substitution → value unchanged (synthesized event, no default).
    expect(value).toBe('click here please');
  });
});
