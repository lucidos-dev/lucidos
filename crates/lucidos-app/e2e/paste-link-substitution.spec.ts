import { test, expect, Locator } from '@playwright/test';
import { navigateToApp, waitForVisibleInput, assertHealthy } from './helpers';

/** Dispatch a real paste event with the given clipboard text, mimicking the
 *  browser's clipboard delivery. Bypasses permission prompts that block real
 *  clipboard reads in Playwright. */
async function pasteText(input: Locator, text: string) {
  await input.evaluate((el: HTMLTextAreaElement, payload: string) => {
    const dt = new DataTransfer();
    dt.setData('text/plain', payload);
    el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
  }, text);
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
      el.setSelectionRange(4, 13); // "yesterday"
    });

    await pasteText(input, ref);

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
      el.setSelectionRange(9, 16); // "article"
    });

    await pasteText(input, '[Original Title](https://vg.no/news/123)');

    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe('Read the [article](https://vg.no/news/123) first.');
  });

  test('falls through to default paste when no selection', async ({ page }) => {
    await navigateToApp(page);
    const input = await waitForVisibleInput(page);

    await input.evaluate((el: HTMLTextAreaElement) => {
      el.focus();
      el.value = '';
      el.setSelectionRange(0, 0);
    });

    // No selection → handler must not preventDefault → textarea stays empty
    // (real paste fills it; our synthesized event has no native default).
    await pasteText(input, 'https://vg.no/article');
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
      el.setSelectionRange(6, 11); // "world"
    });

    await pasteText(input, 'localhost:3000');
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
      el.setSelectionRange(4, 12); // "array[0]"
    });

    await pasteText(input, 'https://vg.no/x');
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
      el.setSelectionRange(6, 10); // "here"
    });

    // Crafted payload: if the handler accepted this verbatim it would emit
    // [here](https://evil/)[real](https://good) — two links, with "here"
    // pointing at the attacker's URL.
    await pasteText(input, 'https://evil/)[real](https://good');
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    // No substitution → value unchanged (synthesized event, no default).
    expect(value).toBe('click here please');
  });
});
