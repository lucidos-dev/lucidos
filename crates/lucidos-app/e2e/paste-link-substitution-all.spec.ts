import { test, expect, Locator } from './fixtures';
import { navigateToApp, waitForVisibleInput, assertHealthy } from './helpers';

async function pasteText(input: Locator, text: string, selection?: [number, number]) {
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

    await pasteText(input, ref, [4, 13]);

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

    await pasteText(input, '[Original Title](https://vg.no/news/123)', [9, 16]);

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

    await pasteText(input, 'localhost:3000', [6, 11]);
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
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

    await pasteText(input, 'https://vg.no/x', [4, 12]);
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

    await pasteText(input, 'https://evil/)[real](https://good', [6, 10]);
    const value = await input.evaluate((el: HTMLTextAreaElement) => el.value);
    expect(value).toBe('click here please');
  });
});
