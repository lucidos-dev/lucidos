import { test as base, request as pwRequest } from '@playwright/test';
import type { APIRequestContext, BrowserContext, Page as PlaywrightPage } from '@playwright/test';
import { HARNESS_DEVICE_ID, registerHarnessDevice } from './harnessDevice';

export { expect } from '@playwright/test';
export type { APIRequestContext, Locator, Page } from '@playwright/test';

type NewContextOptions = Parameters<typeof pwRequest.newContext>[0];

/** An `APIRequestContext` that identifies itself as the harness device.
 *
 *  A standalone context shares nothing with the page, so it carries no
 *  `x-lucidos-device-id` and `api::mutating_gate` refuses every mutation it
 *  makes (ADR 0169). The one builder behind both the `request` fixture and the
 *  `request.newContext` export below, so the two cannot drift.
 */
async function newIdentifiedContext(options?: NewContextOptions): Promise<APIRequestContext> {
  const ctx = await pwRequest.newContext({
    ignoreHTTPSErrors: true,
    ...options,
    extraHTTPHeaders: {
      'x-lucidos-device-id': HARNESS_DEVICE_ID,
      ...options?.extraHTTPHeaders,
    },
  });
  await registerHarnessDevice(ctx);
  return ctx;
}

/** `request.newContext`, pre-identified, under the name specs already import.
 *
 *  Deliberately NOT a spread of Playwright's `request`: `newContext` lives on
 *  the prototype, so `{ ...pwRequest }` copies two private fields and no
 *  methods. Naming the one member we wrap says what is actually available.
 */
export const request = { newContext: newIdentifiedContext };

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
  // Every spec taking `{ request }` gets a context that identifies itself. See
  // `harnessDevice.ts` for why the harness registers rather than the gate
  // being narrowed.
  request: async ({ baseURL }, use) => {
    const ctx = await newIdentifiedContext({ baseURL });
    try {
      await use(ctx);
    } finally {
      await ctx.dispose();
    }
  },
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
