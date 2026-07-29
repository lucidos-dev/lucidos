import { test as base } from '@playwright/test';
import type { BrowserContext, Page as PlaywrightPage } from '@playwright/test';

export { expect, request } from '@playwright/test';
export type { APIRequestContext, Locator, Page } from '@playwright/test';

const WEBKIT_CONTEXT_PREFLIGHT_ATTEMPTS = 3;
const WEBKIT_CONTEXT_PREFLIGHT_TIMEOUT_MS = 10_000;
const WEBKIT_CONTEXT_CLOSE_GRACE_MS = 1_000;
const WEBKIT_CONTEXT_PREFLIGHT_PATH = '/api/v1/health';

type ContextFactory = () => Promise<{
  context: BrowserContext;
  close: () => Promise<void>;
}>;

async function closeWithGrace(close: () => Promise<void>): Promise<boolean> {
  const closePromise = close().catch(() => {
    // Preserve the original preflight failure.
  });
  const timeoutPromise = new Promise<'timeout'>((resolve) => {
    setTimeout(() => resolve('timeout'), WEBKIT_CONTEXT_CLOSE_GRACE_MS);
  });

  return (await Promise.race([closePromise.then(() => 'closed' as const), timeoutPromise])) === 'closed';
}

async function closePageQuietly(page: PlaywrightPage): Promise<void> {
  await closeWithGrace(() => page.close());
}

async function preflightWebKitContext(context: BrowserContext, baseURL: string | undefined): Promise<void> {
  if (!baseURL) throw new Error('mobile-webkit context preflight requires Playwright baseURL');

  const page = await context.newPage();
  try {
    const url = new URL(WEBKIT_CONTEXT_PREFLIGHT_PATH, baseURL).toString();
    await page.goto(url, { waitUntil: 'commit', timeout: WEBKIT_CONTEXT_PREFLIGHT_TIMEOUT_MS });
  } finally {
    await closePageQuietly(page);
  }
}

export const test = base.extend({
  context: async ({ browserName, baseURL, _contextFactory }, use, testInfo) => {
    const createContext = _contextFactory as ContextFactory;
    const isMobileWebKit = browserName === 'webkit' && testInfo.project.name === 'mobile-webkit';

    if (!isMobileWebKit) {
      const ready = await createContext();
      try {
        await use(ready.context);
      } finally {
        await ready.close();
      }
      return;
    }

    let ready: Awaited<ReturnType<ContextFactory>> | undefined;
    let lastErr: unknown;
    for (let attempt = 1; attempt <= WEBKIT_CONTEXT_PREFLIGHT_ATTEMPTS; attempt += 1) {
      const candidate = await createContext();
      try {
        await preflightWebKitContext(candidate.context, baseURL);
        ready = candidate;
        break;
      } catch (err) {
        lastErr = err;
        const closed = await closeWithGrace(candidate.close);
        if (attempt < WEBKIT_CONTEXT_PREFLIGHT_ATTEMPTS) {
          console.warn(
            `[mobile-webkit context preflight] discarded context after failed localhost commit (${attempt}/${WEBKIT_CONTEXT_PREFLIGHT_ATTEMPTS})`,
          );
          if (!closed) {
            console.warn('[mobile-webkit context preflight] failed context close still pending; trying a fresh context');
          }
        }
      }
    }

    if (!ready) throw lastErr;

    try {
      await use(ready.context);
    } finally {
      await ready.close();
    }
  },
});
