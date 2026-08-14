import { Page, expect, Locator } from '@playwright/test';
import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

const WORKSPACE = resolve(process.env.E2E_WORKSPACE ?? `${process.env.HOME}/workspaces/e2e-test`);

/** CSS selector for the body of a rendered user message (initiator panel).
 *  Centralized so a UI rename only requires changing this one constant. */
export const USER_MSG_SELECTOR = '.initiator-panel-user .initiator-body';

/** Drawer rows for compose drafts share the `.thread-row` class and a
 *  `data-thread-nav` attr with real thread rows. A test that wants a real
 *  thread must filter drafts out: the draft variants also carry
 *  `compose-draft-row` and `data-draft-id`. */
export const REAL_THREAD_ROW = '.thread-row:not(.compose-draft-row)';
export const REAL_THREAD_NAV = '[data-thread-nav]:not([data-draft-id])';

/** Start of the thread-drawer toggle's `aria-label`. Always match it as a
 *  PREFIX. `ThreadToggleButton` appends " (N needing attention)" whenever the
 *  thread list is hidden and a thread awaits the user, so an exact-match
 *  selector silently stops resolving. */
export const DRAWER_TOGGLE_LABEL = 'Show or hide thread drawer';

/** Locator for the first physically visible user-message body (dual-layout safe). */
export function userMessageBody(page: Page): Locator {
  return page.locator(`${USER_MSG_SELECTOR}:visible`).first();
}

/** Check if viewport is mobile-sized (matches CSS breakpoint at 768px) */
export function isMobileViewport(page: Page): boolean {
  const vp = page.viewportSize();
  return vp ? vp.width < 769 : false;
}

/** Navigate to a mobile pane by name. No-op on desktop, or when already there.
 *  Re-clicks the dot inside the wait loop, so a click absorbed by a concurrent
 *  re-render is retried. `polling: 250` caps re-clicks at 4/sec instead of the
 *  rAF default, to avoid event-storming Preact while the pane settles. */
export async function ensureMobileView(page: Page, viewName: 'thread' | 'threads' | 'content'): Promise<void> {
  if (!isMobileViewport(page)) return;
  await page.waitForFunction((target) => {
    const header = document.querySelector('.app-header');
    if (header?.getAttribute('data-mobile-view') === target) return true;
    const dot = document.querySelector(`button.mobile-dot[aria-label="${target} view"]`);
    if (dot) (dot as HTMLElement).click();
    return false;
  }, viewName, { timeout: 10_000, polling: 250 });
}

/** On mobile, navigate to the thread pane (pane 1). No-op on desktop. */
export async function ensureOnThreadPane(page: Page): Promise<void> {
  await ensureMobileView(page, 'thread');
}

export function getBaseUrl(): string {
  const portsFile = resolve(WORKSPACE, '.lucidos/ports');
  if (!existsSync(portsFile)) {
    throw new Error(`Ports file missing: ${portsFile}`);
  }
  const content = readFileSync(portsFile, 'utf-8');
  const match = content.match(/VITE_PORT=(\d+)/);
  if (!match) throw new Error('VITE_PORT not found');
  return `https://localhost:${match[1]}`;
}

/** Wait for a physically visible prompt input (dual-layout safe).
 *  At mobile viewports the desktop layout is `display: none`, so `.first()` can
 *  pick the hidden one. Wait for any prompt input to become visible, then
 *  return the visible locator. */
export async function waitForVisibleInput(page: Page, timeout = 30_000): Promise<Locator> {
  await page.waitForFunction(() => {
    const els = document.querySelectorAll('[data-role="prompt-input"]');
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout });

  return page.locator('[data-role="prompt-input"]:visible').first();
}

/** Navigate to `url` (main-document commit) with a BOUNDED per-attempt timeout
 *  and a retry. THE canonical way to load the app in e2e: specs route their
 *  `page.goto` through this rather than calling `page.goto` directly.
 *
 *  On the `mobile-webkit` project the FIRST navigation in a fresh context
 *  intermittently wedges. Both causes, and everything ruled out, are in
 *  docs/e2e-test-decisions.md § "mobile-webkit navigation wedge". The primary
 *  one is fixed at the source in playwright.config.ts, and a pre-commit
 *  cold-context stall is handled by the preflight in e2e/fixtures.ts.
 *
 *  What is left for this helper: CAP any later pre-commit hang, failing at
 *  ATTEMPTS*timeout rather than the full test budget, and re-navigate once.
 *  Waiting only for the response commit keeps a post-commit lifecycle stall
 *  from reading as a failed app load, so callers assert real readiness
 *  afterwards. */
export async function gotoWithRetry(page: Page, url = '/'): Promise<void> {
  const ATTEMPTS = 2;
  const PER_ATTEMPT_TIMEOUT_MS = 30_000;
  let lastErr: unknown;
  for (let attempt = 1; attempt <= ATTEMPTS; attempt++) {
    try {
      await page.goto(url, { waitUntil: 'commit', timeout: PER_ATTEMPT_TIMEOUT_MS });
      return;
    } catch (err) {
      lastErr = err;
      // Stalled before the main response committed. Re-navigate once; a sticky
      // freeze still falls through to the whole-test fresh-context retry.
    }
  }
  throw lastErr;
}

export async function navigateToApp(page: Page): Promise<void> {
  await gotoWithRetry(page, '/');
  await ensureOnThreadPane(page);
  await waitForVisibleInput(page);
}

/** Wait until the page's SSE event stream is open.
 *  Required before tests emit transient engine events that are delivered only
 *  over SSE, such as `/api/v1/ui/navigate` NavigationRequested events. */
export async function waitForEventStream(page: Page, timeout = 10_000): Promise<void> {
  await page.waitForFunction(() => {
    return document.documentElement.dataset.lucidosEventStream === 'connected';
  }, undefined, { timeout });
}

export async function sendMessage(page: Page, text: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(text);
  // On mobile viewports (<=768px), PromptInput disables Enter-to-submit
  // (Enter inserts newline for the on-screen keyboard). Click the send button instead.
  if (isMobileViewport(page)) {
    await clickVisibleElement(page, 'button[aria-label="Send message"]');
  } else {
    await input.press('Enter');
  }
}

/** Wait for a response to appear and finish streaming (handles dual-layout) */
export async function waitForResponse(page: Page, timeout = 90_000): Promise<Locator> {
  // Dual-layout: find a physically visible response-content element
  await page.waitForFunction(() => {
    const els = document.querySelectorAll('.response-content');
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout });

  const allResponses = page.locator('.response-content');
  const count = await allResponses.count();
  let response = allResponses.first();
  for (let i = 0; i < count; i++) {
    if (await allResponses.nth(i).isVisible().catch(() => false)) {
      response = allResponses.nth(i);
      break;
    }
  }

  // Wait for exchange status labels to stop showing "Working"/"Requesting"
  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    if (labels.length === 0) return true;
    for (const label of labels) {
      const rect = label.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) continue;
      const text = label.textContent ?? '';
      if (text.includes('Working') || text.includes('Requesting')) return false;
    }
    return true;
  }, undefined, { timeout });

  return response;
}

/** Leave the step log ON, which is the door to everything a step row carries.
 *
 *  Steps SHOW by default (`stepsExpanded`, persisted in localStorage), so on a
 *  fresh context this is a no-op. It asks for the control in any state and
 *  clicks conditionally: an unconditional click would TURN STEPS OFF on the
 *  ordinary run, and a locator pinned to `aria-pressed="false"` would time out
 *  there.
 *
 *  It does NOT wait for a step to exist. Every response turn carries the
 *  control, whatever it holds, so keep asserting on the step row itself
 *  afterwards. */
export async function revealSteps(page: Page, timeout = 30_000): Promise<void> {
  const toggle = page
    .locator('.response-controls [data-role="toggle-steps"]:visible')
    .first();
  await expect(toggle).toBeVisible({ timeout });
  if (await toggle.getAttribute('aria-pressed') === 'false') await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
}

/** Wait for at least one physically visible element matching a selector (dual-layout safe). */
export async function waitForVisibleElement(page: Page, selector: string, timeout = 5_000): Promise<void> {
  await page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => el.getBoundingClientRect().width > 0);
  }, selector, { timeout });
}

/** Wait for a visible element then click it (dual-layout safe). */
export async function waitAndClick(page: Page, selector: string, text?: string, timeout = 5_000): Promise<void> {
  await waitForVisibleElement(page, selector, timeout);
  await clickVisibleElement(page, selector, text);
}

/** Click a content-header action by its ACTION class (`.file-edit-btn`,
 *  `.diff-whole-file-toggle`) wherever progressive collapse put it.
 *
 *  The content header folds its leading actions into a `⋯` overflow menu when
 *  the row runs out of room for the title (`useHeaderActionCollapse`). On a
 *  phone an ordinary long title can fold EVERY action behind `⋯`. A test
 *  waiting on the bare header button then times out on a layout behaving
 *  exactly as designed. Placement is the layout's business; the test's
 *  business is that the action works.
 *
 *  Both renderings carry the action class, so one selector finds it either way.
 *  This still fails loudly when the action is genuinely absent: neither
 *  placement appears and the wait times out. */
export async function clickHeaderAction(page: Page, actionSelector: string, timeout = 10_000): Promise<void> {
  const menuRow = `.thread-overflow-item${actionSelector}`;
  // Settled = the action has a placement: its own header button, or the `⋯`
  // trigger that would hold it. Waiting on either avoids racing the collapse
  // hook's layout-effect measurement.
  await page.waitForFunction(({ sel }) => {
    const anyVisible = (s: string) =>
      Array.from(document.querySelectorAll(s)).some(el => el.getBoundingClientRect().width > 0);
    return anyVisible(sel) || anyVisible('.content-header-more');
  }, { sel: actionSelector }, { timeout });

  if (await clickVisibleElement(page, actionSelector)) return;

  if (!await clickVisibleElement(page, '.content-header-more')) {
    throw new Error(`clickHeaderAction: "${actionSelector}" is not in the header and the overflow trigger is not clickable`);
  }
  await waitForVisibleElement(page, menuRow, timeout);
  if (!await clickVisibleElement(page, menuRow)) {
    throw new Error(`clickHeaderAction: "${actionSelector}" is not in the header nor in the overflow menu`);
  }
}

/** Is a content-header action OFFERED to the reader, in EITHER placement?
 *
 *  The reading counterpart of `clickHeaderAction`, and the sharper half of the
 *  same problem. A folded action has no header button, so a bare
 *  `expect('.the-action').toHaveCount(0)` is satisfied by a folded action
 *  exactly as by an absent one. A "this surface does not offer that control"
 *  assertion written that way stops being able to fail.
 *
 *  Leaves no state behind: the `⋯` menu is only opened when the action has no
 *  header button, and is closed again through its own trigger before returning. */
export async function headerActionOffered(page: Page, actionSelector: string, timeout = 10_000): Promise<boolean> {
  // Settle first, for the reason clickHeaderAction settles: the collapse hook
  // measures in a layout effect, so before it has run neither placement exists
  // and every answer here would be a false negative.
  await page.waitForFunction(({ sel }) => {
    const anyVisible = (s: string) =>
      Array.from(document.querySelectorAll(s)).some(el => el.getBoundingClientRect().width > 0);
    return anyVisible(sel) || anyVisible('.content-header-more');
  }, { sel: actionSelector }, { timeout }).catch(() => {
    // Neither placement ever appeared, which IS the answer when a caller is
    // asking whether an action is offered. Fall through to the reads below.
  });

  if (await page.locator(`${actionSelector}:visible`).count() > 0) return true;
  if (!await clickVisibleElement(page, '.content-header-more')) return false;
  await waitForVisibleElement(page, '.thread-overflow-item', timeout);
  const offered = await page.locator(`.thread-overflow-item${actionSelector}`).count() > 0;
  // Close through the trigger rather than Escape: the trigger is the menu's
  // anchor, so its own handler toggles the menu shut, and nothing else on the
  // Escape stack is disturbed.
  await clickVisibleElement(page, '.content-header-more');
  await page.waitForFunction(
    () => document.querySelectorAll('.thread-overflow-item').length === 0,
    undefined, { timeout },
  );
  return offered;
}

/** The id of the thread the app currently has focused, read from the same
 *  `localStorage` key the app persists it under.
 *
 *  This is the identity-safe way to answer "which thread did I just create?"
 *  after a `sendMessage` + `waitForResponse`. Reading it off the drawer with a
 *  positional `.first()` is unsafe, for the reason `threadRowFor` documents
 *  below. Throws when nothing is focused, since a caller asking for the id
 *  always believes a thread is. */
export async function focusedThreadId(page: Page): Promise<string> {
  const id = await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'));
  if (!id) throw new Error('focusedThreadId: no thread is focused (lucidos-focused-thread is unset)');
  return id;
}

/** Selector for ONE specific thread's drawer row.
 *
 *  Use this, never `REAL_THREAD_ROW` plus a positional `.first()`, whenever a
 *  test means "the thread I just created". Positional row selection is unsafe
 *  in this suite: `clearAllThreads()` truncates only the `thread_summaries`
 *  PROJECTION, so a coding-agent session left running by an EARLIER spec
 *  re-inserts its own row with `last_activity = NOW()`. That sorts it ABOVE the
 *  row this test just made. `.first()` then clicks a foreign thread, and every
 *  later assertion silently measures the wrong one.
 *
 *  Keys on `data-flip-id`, which `ThreadDrawer.tsx` stamps on every row wrapper
 *  for its FLIP animation and keyboard nav. That is the one stable per-thread
 *  hook the list carries, so a rename there must move this with it. */
export function threadRowFor(threadId: string): string {
  return `[data-flip-id="${threadId}"] .thread-row`;
}

/** Click a SPECIFIC thread's drawer row (dual-layout safe, identity-based).
 *  Waits for that row to render, then clicks it through the same touch-routing
 *  bypass `clickVisibleElement` uses. Never falls back to another row: a row
 *  that does not appear is itself the bug, and it throws naming the thread.
 *
 *  The wait is a PRECONDITION, not the assertion a caller is testing, so it is
 *  deliberately generous. A WebContent paint stall must not turn "click my row"
 *  into a flake. */
export async function clickThreadRow(page: Page, threadId: string, timeout = 10_000): Promise<void> {
  const selector = threadRowFor(threadId);
  try {
    await waitForVisibleElement(page, selector, timeout);
  } catch (err) {
    throw new Error(`Drawer row for thread ${threadId} never became visible: ${(err as Error).message}`);
  }
  if (!await clickVisibleElement(page, selector)) {
    throw new Error(`Drawer row for thread ${threadId} was not clickable`);
  }
}

/** Click the first physically visible element matching a selector (dual-layout safe).
 *  Optionally filter by text content. Returns whether an element was clicked. */
export async function clickVisibleElement(page: Page, selector: string, text?: string): Promise<boolean> {
  return page.evaluate(({ sel, txt }) => {
    const els = document.querySelectorAll(sel);
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        if (txt && !(el.textContent ?? '').includes(txt)) continue;
        (el as HTMLElement).click();
        return true;
      }
    }
    return false;
  }, { sel: selector, txt: text });
}

/** Open the unified Filter panel and pick a drawer view by its row label
 *  ("All" | "Needs attention" | "Review" | "Running" | "Drafts"). The rows live
 *  in the panel's Status section; picking one applies it and closes the panel.
 *  The panel renders inside the thread drawer pane, which is the same component
 *  on both layouts, so this is dual-layout safe: the single Filter button
 *  (`aria-label="Filter threads"`) lives in both threads headers. Throws if the
 *  button or row isn't visible. */
export async function openDrawerView(page: Page, label: string): Promise<void> {
  const opened = await clickVisibleElement(page, 'button[aria-label="Filter threads"]');
  if (!opened) throw new Error('Filter threads button not visible');
  const picked = await clickVisibleElement(page, '.thread-filter-panel .drawer-view-option', label);
  if (!picked) throw new Error(`Drawer view option "${label}" not visible`);
}

/** Click compose button to start a new thread (dual-layout safe).
 *  On mobile the compose button navigates to thread pane automatically.
 *
 *  The two layouts reach it differently. The desktop thread-pane header carries
 *  New thread as an icon button. Both mobile headers have no room for it and
 *  keep it inside the Lucidos menu as a `.brand-menu-item`.
 *
 *  The menu route is gated on the mobile viewport, because that is the only
 *  place the menu HAS that item. Running it on desktop would open the Lucidos
 *  menu, find nothing, and leave it standing over the app for the rest of the
 *  spec. A miss stays non-fatal: the caller may already be on the compose view,
 *  in which case the waits below pass anyway. */
export async function newThread(page: Page): Promise<void> {
  const clicked = await clickVisibleElement(page, 'button[aria-label="New thread"]');
  if (!clicked && isMobileViewport(page)) {
    await clickVisibleElement(page, 'button[aria-label^="Lucidos menu"]');
    if (!await clickVisibleElement(page, '.brand-menu-item', 'New thread')) {
      await page.keyboard.press('Escape'); // never leave the menu standing open
    }
  }
  await ensureOnThreadPane(page);
  // Wait for compose/create view (no existing messages visible)
  await page.waitForFunction((sel) => {
    return !Array.from(document.querySelectorAll(sel)).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, USER_MSG_SELECTOR, { timeout: 5_000 });
  await waitForVisibleInput(page, 5_000);
}

/** Open the thread list — on mobile navigates to threads pane, on desktop toggles the drawer */
export async function openThreadDrawer(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    await ensureMobileView(page, 'threads');
    await page.waitForFunction(() => {
      const drawer = document.querySelector('.mobile-threads-pane .thread-drawer');
      return drawer && drawer.getBoundingClientRect().width > 0;
    }, undefined, { timeout: 5_000 });
    return;
  }
  const isOpen = await page.evaluate(() => {
    const drawers = document.querySelectorAll('.thread-drawer:not(.thread-drawer-collapsed)');
    return Array.from(drawers).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  });
  if (!isOpen) {
    // Scoped to the desktop header's own slot rather than bare, because the
    // mobile header's copy stays mounted under a desktop viewport.
    //
    // Match the label as a PREFIX, never exact. ThreadToggleButton appends the
    // needs-attention count to its own aria-label, exactly while the thread
    // list is hidden. That is every case this branch runs in, so an `=` match
    // finds nothing the moment any thread awaits the user.
    await page.locator(`.thread-toggle-slot button[aria-label^="${DRAWER_TOGGLE_LABEL}"]`).click();
  }
  // Wait for the drawer's open width-transition to SETTLE, not merely to be
  // non-zero. Returning at width > 0 catches the drawer mid-slide, where the
  // still-narrow title column wraps a long title to one character per line.
  // Geometry assertions then read a degenerate layout. Poll until the width is
  // stable across two frames and past the collapsed strip.
  await page.waitForFunction(() => {
    const drawer = Array.from(document.querySelectorAll('.thread-drawer:not(.thread-drawer-collapsed)'))
      .find(el => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      });
    if (!drawer) return false;
    const w = drawer.getBoundingClientRect().width;
    const win = window as unknown as { __luDrawerW?: number };
    const prev = win.__luDrawerW;
    win.__luDrawerW = w;
    return w > 100 && prev !== undefined && Math.abs(prev - w) < 0.5;
  }, undefined, { timeout: 5_000 });
}

export function uniqueMessage(prefix = 'e2e-test'): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

/** Blur the currently focused element — useful when auto-focus hides the mobile header. */
export async function blurActiveElement(page: Page): Promise<void> {
  await page.evaluate(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  });
}

/** Get the top position of the app header (dual-layout safe). Returns -999 if not found. */
export async function getHeaderTop(page: Page): Promise<number> {
  return page.evaluate(() => {
    const header = document.querySelector('.app-header');
    return header ? header.getBoundingClientRect().top : -999;
  });
}

/** Opt out of the default-ON "Keep header visible" mobile preference, so the
 *  header's hide-on-scroll and hide-on-keyboard-open behavior is exercisable.
 *  With the pin on, the header never slides off and the hide assertions time
 *  out. Set the GLOBAL pref to 'false' BEFORE navigating so the page boots with
 *  hide enabled: with `device_id` omitted, the app's device-scoped preference
 *  load merges the global value. Must be called before `navigateToApp`. */
export async function disableMobileHeaderSticky(page: Page): Promise<void> {
  const res = await page.request.put('/api/v1/preferences?key=mobile_header_sticky', {
    data: { value: 'false' },
  });
  expect(res.ok()).toBeTruthy();
}

/** Force-ON the default "Keep header visible" pin. `mobile_header_sticky` is a
 *  GLOBAL preference, and the e2e DB is reset only between projects. So a test
 *  that called `disableMobileHeaderSticky` leaks the off state into later tests
 *  assuming the pinned default. A test depending on the pinned header calls
 *  this in its beforeEach BEFORE navigating, so it boots pinned whatever the
 *  order. Pairs with `disableMobileHeaderSticky`. */
export async function enableMobileHeaderSticky(page: Page): Promise<void> {
  const res = await page.request.put('/api/v1/preferences?key=mobile_header_sticky', {
    data: { value: 'true' },
  });
  expect(res.ok()).toBeTruthy();
}

export async function assertHealthy(page: Page): Promise<void> {
  const response = await page.request.get('/api/v1/health');
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body.status).toBe('ok');
}

/** Every open menu's option rows. The shared `Dropdown` portals its panel to
 *  <body> (clearing the header's stacking context), so an option is NOT under
 *  the trigger's wrapper. Addressing them globally is safe because the overlay
 *  dismiss contract allows only one open menu at a time. */
const MENU_OPTION = '.dropdown-menu .dropdown-option';

/** Drive a shared `Dropdown` (components/shared/Dropdown.tsx): open the
 *  trigger inside `rootSelector`, click the option containing `optionLabel`,
 *  and wait for the menu to close. Failures throw at the pick, because silently
 *  proceeding with the previous value sends the test down a minutes-long
 *  wrong-path timeout. Waiting on menu CLOSE rather than on the trigger label
 *  is deliberate: the trigger's hidden .dropdown-sizer spans contain EVERY
 *  option label, so a label assertion would always pass.
 *
 *  Only the TRIGGER lives under `rootSelector`; the options come from
 *  `MENU_OPTION` above. */
export async function pickDropdownOption(page: Page, rootSelector: string, optionLabel: string): Promise<void> {
  const opened = await clickVisibleElement(page, `${rootSelector} .dropdown-trigger`);
  if (!opened) throw new Error(`pickDropdownOption: no visible ${rootSelector} .dropdown-trigger`);
  // The menu renders on the next Preact commit — wait for the option to land.
  await page.waitForFunction(({ sel, label }) => {
    const opts = document.querySelectorAll(sel);
    return Array.from(opts).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(label);
    });
  }, { sel: MENU_OPTION, label: optionLabel }, { timeout: 5_000 });
  const picked = await clickVisibleElement(page, MENU_OPTION, optionLabel);
  if (!picked) throw new Error(`pickDropdownOption: option "${optionLabel}" not clickable for ${rootSelector}`);
  await page.waitForFunction((sel) => {
    const opts = document.querySelectorAll(sel);
    return !Array.from(opts).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, MENU_OPTION, { timeout: 3_000 });
}

/** Pick an option in the compose destination picker (dual-layout safe).
 *  Defaults to 'Lucidos source', which spawns a Lucidos-internal coding-agent
 *  thread on the next send. Pass another option label to target it instead. */
export async function pickComposeDestination(page: Page, optionLabel = 'Lucidos source'): Promise<void> {
  await ensureOnThreadPane(page);
  await pickDropdownOption(page, '.compose-destination-picker', optionLabel);
}

/** Send a follow-up message in an existing thread */
export async function sendFollowUp(page: Page, text: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(text);
  if (isMobileViewport(page)) {
    await clickVisibleElement(page, 'button[aria-label="Send message"]');
  } else {
    await input.press('Enter');
  }
}

/** Count visible exchanges (user message + response pairs) — handles dual-layout */
export async function countExchanges(page: Page): Promise<number> {
  return page.evaluate(() => {
    const exchanges = document.querySelectorAll('.chat-exchange');
    return Array.from(exchanges).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length;
  });
}

/** Wait for at least N visible exchanges to appear */
export async function waitForExchangeCount(page: Page, minCount: number, timeout = 30_000): Promise<void> {
  await page.waitForFunction((min) => {
    const exchanges = document.querySelectorAll('.chat-exchange');
    return Array.from(exchanges).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length >= min;
  }, minCount, { timeout });
}

/** Wait for the WaitingBanner action panel to appear with specific button text */
export async function waitForActionPanel(page: Page, buttonText: string, timeout = 120_000): Promise<Locator> {
  await ensureOnThreadPane(page);
  await page.waitForFunction((text) => {
    const panels = document.querySelectorAll('.thread-action-buttons');
    return Array.from(panels).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes(text);
    });
  }, buttonText, { timeout });
  return page.locator('.thread-action-buttons:visible').first();
}

/** Click a WaitingBanner change action by label, transparently across the two
 *  banner shapes. When an Apply action is present the banner is a split button
 *  (all viewports): the primary Apply face stays a direct button — locate that
 *  one directly, not via this helper — while Diff / Discard / Archive live in
 *  the caret menu. When there's no Apply (e.g. an idle CC thread with a diff but
 *  no pending change) the actions render as their own buttons. Tries the direct
 *  button first; otherwise opens the caret menu and clicks the matching item. */
export async function clickChangeAction(
  page: Page,
  label: 'Discard' | 'Diff' | 'Archive',
  timeout = 15_000,
): Promise<void> {
  const direct = page.locator(`.thread-action-buttons:visible button.action-btn:has-text("${label}")`).first();
  if (await direct.isVisible().catch(() => false)) {
    await direct.click();
    return;
  }
  // Mobile split button: the action lives behind the caret.
  const caret = page.locator('.thread-action-buttons:visible .split-button-caret').first();
  await expect(caret).toBeVisible({ timeout });
  await caret.click();
  await page.locator(`.split-button-menu:visible button:has-text("${label}")`).first().click();
}

/** Resolve only on the LAST visible status label leaving Working/Requesting.
 *  Earlier turns may still show idle Done/Diff panels mid-stream of a later
 *  turn, so a "any panel exists" check would return early. */
export async function waitForCCToFinish(page: Page, timeout = 120_000): Promise<void> {
  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    const visible = Array.from(labels).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
    if (visible.length === 0) return true;
    const last = visible[visible.length - 1];
    const text = last.textContent ?? '';
    return !(text.includes('Working') || text.includes('Requesting'));
  }, undefined, { timeout });
}

/** Wait for streaming to start (visible response-content with text above minLength) */
export async function waitForStreamingToStart(page: Page, minLength = 5, timeout = 30_000): Promise<void> {
  await page.waitForFunction((min) => {
    const els = document.querySelectorAll('.response-content');
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').length > min;
    });
  }, minLength, { timeout });
}

/** Wait for CC to start working (status label shows Working/Requesting) */
export async function waitForCCToStart(page: Page, timeout = 60_000): Promise<void> {
  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    return Array.from(labels).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      const text = el.textContent ?? '';
      return text.includes('Working') || text.includes('Requesting');
    });
  }, undefined, { timeout });
}

/** Poll the commands API until the CC session is active (in the session map).
 *  The frontend shows "Requesting" optimistically before the backend has fully
 *  created the session (worktree setup, DB lookups, etc.). Without polling,
 *  queries hit the cache fallback which returns stale values. */
export async function waitForActiveSession(page: Page, threadId: string, timeout = 30_000): Promise<Record<string, unknown>> {
  let cmdData: Record<string, unknown> = {};
  await expect(async () => {
    const cmdResp = await page.request.get(`/api/v1/claude-code/commands?thread_id=${threadId}`);
    expect(cmdResp.ok()).toBeTruthy();
    cmdData = await cmdResp.json();
    expect(cmdData.has_active_session).toBe(true);
  }).toPass({ timeout, intervals: [500, 1000, 2000] });
  return cmdData;
}

/** Assert that all given markers appear in visible user-message body elements.
 *  POLLS until every marker is visible rather than snapshotting once. A
 *  just-confirmed follow-up swaps its optimistic pending row for the persisted
 *  exchange on the next Preact flush. A single evaluate() can therefore read
 *  the one-frame gap before that body repaints. A genuinely missing message
 *  still fails loudly when the poll times out, and the recomputed `missing`
 *  set names which markers. */
export async function assertUserMessagesVisible(page: Page, markers: string[], timeout = 15_000): Promise<void> {
  await expect(async () => {
    const missing = await page.evaluate(({ sel, ms }) => {
      const visibleTexts: string[] = [];
      document.querySelectorAll(sel).forEach(el => {
        const rect = el.getBoundingClientRect();
        if (rect.width > 0 && rect.height > 0) visibleTexts.push(el.textContent ?? '');
      });
      return ms.filter(m => !visibleTexts.some(t => t.includes(m)));
    }, { sel: USER_MSG_SELECTOR, ms: markers });
    expect(missing, `User messages not visible: ${missing.join(', ')}`).toEqual([]);
  }).toPass({ timeout });
}

/** Count visible response-content elements with non-empty text */
export async function countVisibleResponses(page: Page): Promise<number> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.response-content');
    return Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    }).length;
  });
}

/** Wait until at least `count` visible response-content elements have non-empty
 *  text. Prefer this over waitForResponse() before a "got N responses"
 *  assertion in a multi-turn test. waitForResponse() only checks that no status
 *  label reads Working or Requesting. Just after a prior turn settles it can
 *  therefore resolve before the next turn streams, leaving the count short. */
export async function waitForVisibleResponseCount(
  page: Page,
  count: number,
  timeout = 90_000,
): Promise<void> {
  await page.waitForFunction((n) => {
    const els = document.querySelectorAll('.response-content');
    return Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    }).length >= n;
  }, count, { timeout });
}

/** Trimmed text of the last visible response-content element, or '' if none. */
export async function getLatestVisibleResponseText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.response-content');
    const visible = Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    });
    return (visible[visible.length - 1]?.textContent ?? '').trim();
  });
}

/** Count visible thread-row elements (handles dual-layout) */
export async function countVisibleThreadRows(page: Page): Promise<number> {
  return page.evaluate(() => {
    const rows = document.querySelectorAll('.thread-row');
    return Array.from(rows).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length;
  });
}

/** Wait for the prompt-area Cancel button, click it, then wait for Canceled
 *  status. Clicking the stop button cancels immediately, with no confirm
 *  dialog, on both chat and Claude Code threads.
 *
 *  The Send-to-Cancel morph is identified by its `aria-label="Cancel"`. The
 *  disabled canceling state shares that label, so `:not(:disabled)` is
 *  load-bearing to hit the actionable stop state. */
export async function cancelStreamingResponse(page: Page): Promise<void> {
  await waitAndClick(page, 'button.send-cancel-morph[aria-label="Cancel"]:not(:disabled)', undefined, 30_000);

  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    return Array.from(labels).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      return (el.textContent ?? '').includes('Canceled');
    });
  }, undefined, { timeout: 30_000 });
}

/** Navigate to the Files panel — handles mobile pane navigation */
export async function openFilesPanel(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    // On mobile, the drawer is only accessible from the content pane (via hamburger).
    // Navigate to content pane first, open the drawer, then click 'Files'.
    await page.evaluate(() => {
      const dot = document.querySelector('button.mobile-dot[aria-label="content view"]');
      if (dot) (dot as HTMLElement).click();
    });
    await page.waitForTimeout(300);
    await clickVisibleElement(page, '.hamburger-panel');
    // Wait for the drawer to open and items to be visible
    await page.waitForFunction(() => {
      const items = document.querySelectorAll('.drawer-item');
      return Array.from(items).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 3_000 });
  }
  await clickVisibleElement(page, '.drawer-item', 'Files');
  await page.waitForFunction(() => {
    const views = document.querySelectorAll('.content-view.active');
    return Array.from(views).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 5_000 });
}

/** The triggers panel's "Add Trigger" card (dual-layout safe, visible copy). */
export function addTriggerCard(page: Page): Locator {
  return page.locator('.list-row-add-card:visible', { hasText: 'Add Trigger' }).first();
}

/** Navigate to the Triggers panel. The nav drawer (with the menu items) is
 *  hidden by default on BOTH layouts and opened via the `.hamburger-panel`
 *  toggle, so open it first, then click 'Triggers'. On mobile the hamburger
 *  lives on the content pane, so swipe there first. Finally waits for the
 *  panel's "Add Trigger" card so callers never click a still-loading list. */
export async function openTriggersPanel(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    await page.evaluate(() => {
      const dot = document.querySelector('button.mobile-dot[aria-label="content view"]');
      if (dot) (dot as HTMLElement).click();
    });
    await page.waitForTimeout(300);
  }
  await clickVisibleElement(page, '.hamburger-panel');
  await page.waitForFunction(() => {
    const items = document.querySelectorAll('.drawer-item');
    return Array.from(items).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 5_000 });
  await clickVisibleElement(page, '.drawer-item', 'Triggers');
  // The Add Trigger card only renders once the triggers list has loaded — wait
  // for it so the create flow doesn't race the projection fetch.
  await expect(addTriggerCard(page)).toBeVisible({ timeout: 10_000 });
}

/** Best-effort dismiss of an idle CC session by clicking Done (dual-layout safe) */
export async function dismissCCSession(page: Page): Promise<void> {
  try {
    await ensureOnThreadPane(page);
    await clickVisibleElement(page, '.thread-action-buttons button.action-btn', 'Archive');
  } catch {
    // CC session may have already ended — not an error
  }
}

/** Wait for a visible `.thread-title-display` (read-only <div>) with non-empty
 *  text (dual-layout safe). */
export async function waitForThreadTitle(page: Page, timeout = 30_000): Promise<void> {
  await page.waitForFunction(() => {
    const els = document.querySelectorAll('.thread-title-display');
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    });
  }, undefined, { timeout });
}

/** Wait for the thread title editor to enter edit mode (wrapper gains
 *  `.is-editing`), returning a locator for the visible input. */
export async function waitForTitleInput(page: Page, timeout = 5_000) {
  await page.waitForFunction(() => {
    const wrappers = document.querySelectorAll('.thread-title-edit.is-editing');
    return Array.from(wrappers).some(w => {
      const rect = w.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout });
  return page.locator('.thread-title-edit.is-editing .thread-title-edit-input').first();
}

/** Height (px) of the visible mobile thread-title-display div, ignoring
 *  the desktop copy. Returns 0 if neither layout's display is rendered. */
export async function getMobileTitleHeight(page: Page): Promise<number> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.mobile-thread-title-row .thread-title-display');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0) return rect.height;
    }
    return 0;
  });
}

/** Visible thread title text from whichever layout's display div is
 *  currently rendered (dual-layout safe). */
export async function getVisibleTitleText(page: Page): Promise<string> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.thread-title-display');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        return (el.textContent ?? '').trim();
      }
    }
    return '';
  });
}

// =====================================================================
// Test-only push-log assertions.
// Backed by GET /api/v1/_test/push-log, which is only mounted when the engine
// is built with the `e2e-test-hooks` cargo feature (see
// system-knowhow/notifications.md §5.4). Used by the §5.3 scenarios in
// notifications.spec.ts to assert "OS push WAS / WAS NOT sent" without
// waiting for actual APNs/FCM delivery.
// =====================================================================

export interface PushLogEntry {
  device_id: string;
  notification_id: string;
  sent_at: string;
  /** The JSON string the real transport would have encrypted and sent. Lets
   *  §5.3 scenarios assert the Declarative Web Push envelope shape as well as
   *  delivery. */
  payload?: string | null;
}

/** Page-scoped fetch, through Playwright's APIRequestContext. Required so the
 *  engine's self-signed localhost cert is trusted via the browser context that
 *  already accepts it. Node's stricter fetch rejects it. */
async function fetchPushLog(
  page: Page,
  params: {
    notificationId?: string;
    deviceId?: string;
  },
): Promise<PushLogEntry[]> {
  const qs: Record<string, string> = {};
  if (params.notificationId) qs.notification_id = params.notificationId;
  if (params.deviceId) qs.device_id = params.deviceId;
  const res = await page.request.get('/api/v1/_test/push-log', { params: qs });
  if (!res.ok()) {
    throw new Error(
      `GET /api/v1/_test/push-log -> ${res.status()}. Is the engine built with --features e2e-test-hooks?`,
    );
  }
  return (await res.json()) as PushLogEntry[];
}

/** Poll the push-log until a row appears for `notificationId` (optionally
 *  scoped to `deviceId`). Returns the row so callers can inspect the
 *  recorded payload (Declarative Web Push envelope shape). Throws if no
 *  row arrives within `timeoutMs`. */
export async function expectPushSent(
  page: Page,
  notificationId: string,
  options: { deviceId?: string; timeoutMs?: number } = {},
): Promise<PushLogEntry> {
  const timeoutMs = options.timeoutMs ?? 2000;
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const log = await fetchPushLog(page, {
      notificationId,
      deviceId: options.deviceId,
    });
    if (log.length > 0) return log[0];
    await new Promise((r) => setTimeout(r, 50));
  }
  throw new Error(
    `expected push_log entry for notification=${notificationId}` +
      (options.deviceId ? ` device=${options.deviceId}` : '') +
      `, none arrived in ${timeoutMs}ms`,
  );
}

/** Wait `waitMs` (give the engine time to NOT push), then assert the
 *  push-log has no row for `notificationId`. Use this when the §2 matrix
 *  says push is suppressed; absence-of-evidence is fine because the
 *  engine's decision is synchronous (PresenceCheck deadline + write or
 *  return). */
export async function expectNoPushSent(
  page: Page,
  notificationId: string,
  waitMs = 500,
): Promise<void> {
  await new Promise((r) => setTimeout(r, waitMs));
  const log = await fetchPushLog(page, { notificationId });
  if (log.length > 0) {
    throw new Error(
      `expected NO push for notification=${notificationId}, but ${log.length} log entries: ` +
        JSON.stringify(log),
    );
  }
}
