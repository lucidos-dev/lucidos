import { Page, APIResponse, expect, Locator, Request } from '@playwright/test';
import { HARNESS_DEVICE_ID, ensurePageDeviceRegistered, registerHarnessDevice } from './harnessDevice';
import { withDeviceStamp } from './harnessStamp';
import { MODAL_EXIT_DRAWING_CLASS } from '../src/components/shared/overlayExitClass';
import { computeMinThreadPaneWidth, THREAD_ROW_LEAD_CONTROLS } from '../src/store/paneMinimums';
import { DASHBOARD_RAIL_SHOWN_KEY } from '../src/store/dashboardRailKeys';
import { DEVICE_ID_KEY } from '../src/utils/deviceIdKey';
import { ANCHOR_GAP } from '../src/hooks/anchorGap';
import { rungSwitchCopy } from '../src/components/chat/rungSwitchCopy';
import { homeThreadIdInDb, psql } from './db-helpers';

async function pageDeviceId(page: Page): Promise<string | null> {
  try {
    return await page.evaluate((key) => localStorage.getItem(key), DEVICE_ID_KEY);
  } catch {
    return null;
  }
}

export async function shellDeviceId(page: Page): Promise<string> {
  let id: string | null = null;
  await expect.poll(async () => {
    id = await pageDeviceId(page);
    return id;
  }, { message: 'the shell registered a device' }).toBeTruthy();
  return id as unknown as string;
}

export function apiRequest(page: Page) {
  type Opts = Parameters<Page['request']['post']>[1];
  const withDevice = async (opts: Opts): Promise<Opts> => {
    let id = await pageDeviceId(page);
    if (!id) {
      await registerHarnessDevice(page.request);
      id = HARNESS_DEVICE_ID;
    } else {
      await ensurePageDeviceRegistered(page.request, id);
    }
    return { ...opts, headers: withDeviceStamp({ 'x-lucidos-device-id': id, ...opts?.headers }) };
  };
  return {
    post: async (url: string, opts?: Opts): Promise<APIResponse> =>
      page.request.post(url, await withDevice(opts)),
    put: async (url: string, opts?: Opts): Promise<APIResponse> =>
      page.request.put(url, await withDevice(opts)),
    delete: async (url: string, opts?: Opts): Promise<APIResponse> =>
      page.request.delete(url, await withDevice(opts)),
    patch: async (url: string, opts?: Opts): Promise<APIResponse> =>
      page.request.patch(url, await withDevice(opts)),
  };
}

export async function setDevicePreference(page: Page, key: string, value: string): Promise<void> {
  let device: string | null = null;
  await expect.poll(async () => (device = await pageDeviceId(page)), {
    message: 'the shell registered a device',
  }).toBeTruthy();
  const res = await apiRequest(page).put(`/api/v1/preferences?key=${key}`, { data: { value, device_id: device } });
  expect(res.ok(), `${key}=${value}`).toBe(true);
  expect((await res.json()).success, `${key}=${value}`).toBe(true);
}

export async function openSettingsView(page: Page, view: string): Promise<void> {
  await waitForEventStream(page);
  const nav = await apiRequest(page).post('/api/v1/ui/navigate', {
    headers: { 'content-type': 'application/json' },
    data: { target: 'settings', params: { settings_view: view } },
  });
  expect(nav.ok(), `POST /api/v1/ui/navigate -> ${nav.status()}`).toBeTruthy();
}

export const USER_MSG_SELECTOR = '.initiator-panel-user .initiator-body';

export const REAL_THREAD_ROW = '.thread-row:not(.compose-draft-row)';
export const REAL_THREAD_NAV = '[data-thread-nav]:not([data-draft-id])';

export const DRAWER_TOGGLE_LABEL = 'Show or hide thread drawer';

export function userMessageBody(page: Page): Locator {
  return page.locator(`${USER_MSG_SELECTOR}:visible`).first();
}

export function isMobileViewport(page: Page): boolean {
  const vp = page.viewportSize();
  return vp ? vp.width < 769 : false;
}

type TouchStep =
  | { type: 'touchStart' | 'touchMove'; x: number; y: number }
  | { type: 'touchEnd' };

export async function touchPress(page: Page, steps: TouchStep[]): Promise<void> {
  const cdp = await page.context().newCDPSession(page);
  await Promise.all(steps.map((s) => cdp.send('Input.dispatchTouchEvent', {
    type: s.type,
    touchPoints: s.type === 'touchEnd' ? [] : [{ x: s.x, y: s.y }],
  })));
}

let settleToken = 0;

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

export async function waitForPaneAtRest(page: Page): Promise<void> {
  if (!isMobileViewport(page)) return;
  const token = ++settleToken;
  await page.waitForFunction((token) => {
    const panes = Array.from(document.querySelectorAll('.mobile-swipe-pane'));
    if (panes.length === 0) return true;
    const track = document.querySelector('.mobile-swipe-track');
    const sliding = !!track && track.getAnimations().length > 0;
    const atRest = !sliding && panes.some(p => Math.abs(p.getBoundingClientRect().left) < 0.5);
    const win = window as unknown as { __luPaneSettle?: { token: number; atRest: boolean } };
    const prev = win.__luPaneSettle;
    win.__luPaneSettle = { token, atRest };
    return atRest && prev?.token === token && prev.atRest;
  }, token, { timeout: 5_000 });
}

export async function ensureOnThreadPane(page: Page): Promise<void> {
  await ensureMobileView(page, 'thread');
}

export { getBaseUrl } from './address';

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
    }
  }
  throw lastErr;
}

export async function navigateToApp(page: Page): Promise<void> {
  await gotoWithRetry(page, '/');
  await ensureOnThreadPane(page);
  await waitForVisibleInput(page);
  await waitForWorkspaceReady(page);
  await leaveColdStartHome(page);
}

export async function leaveColdStartHome(page: Page): Promise<void> {
  const home = homeThreadIdInDb();
  if (!home) return;
  const focusKey = 'lucidos-focused-thread';
  let focused: string | null = null;
  for (let waited = 0; waited < 10_000 && focused === null; waited += 100) {
    focused = await page.evaluate((key) => localStorage.getItem(key), focusKey);
    if (focused === null) await page.waitForTimeout(100);
  }
  if (focused !== home) return;
  await newThread(page);
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await expect(page.locator('html[data-keyboard-active]')).toHaveCount(0);
  await waitForHeaderAtRest(page);
}

export async function waitForHeaderAtRest(page: Page): Promise<void> {
  if (!isMobileViewport(page)) return;
  const token = ++settleToken;
  await page.waitForFunction((token) => {
    const header = document.querySelector('.app-header');
    const atRest = !header || !header.getAnimations()
      .some(a => (a as CSSTransition).transitionProperty === 'translate');
    const win = window as unknown as { __luHeaderSettle?: { token: number; atRest: boolean } };
    const prev = win.__luHeaderSettle;
    win.__luHeaderSettle = { token, atRest };
    return atRest && prev?.token === token && prev.atRest;
  }, token, { timeout: 5_000 });
}

export async function waitForDashboardReady(dashboard: Locator): Promise<void> {
  const live = dashboard.locator('.loading-fade > .loading-fade-content > :is(.dashboard-grid:not(.is-settling), .dashboard-empty)');
  await expect(live).toHaveCount(1);
  await expect(dashboard.locator('.loading-fade-skeleton:not(.loading-fade-out)')).toHaveCount(0);
}

export async function waitForWorkspaceReady(page: Page): Promise<void> {
  await page.waitForFunction(
    () => document.querySelector('.boot-splash:not(.boot-splash-leaving)') === null,
    undefined,
    { timeout: 30_000 },
  );
}

export async function waitForModalExitFade(page: Page): Promise<void> {
  await expect(page.locator(`.${MODAL_EXIT_DRAWING_CLASS}`)).toHaveCount(0);
}

export async function disarmFollowSeed(page: Page): Promise<void> {
  await page.addInitScript(() => {
    localStorage.setItem('lucidos-follow-live-edge', 'false');
  });
}

export async function hideDashboardRail(page: Page): Promise<void> {
  await page.addInitScript((key) => {
    localStorage.setItem(key, 'false');
  }, DASHBOARD_RAIL_SHOWN_KEY);
}

export async function mockChangesState(
  page: Page,
  state: unknown,
  opts: { changesDelayMs?: number } = {},
): Promise<{ reads: () => number }> {
  let reads = 0;
  await page.route('**/api/v1/changes*', async (route) => {
    reads++;
    if (opts.changesDelayMs) await new Promise((r) => setTimeout(r, opts.changesDelayMs));
    await route.fulfill({ status: 200, contentType: 'application/json', body: JSON.stringify(state) });
  });
  await page.route('**/api/v1/bootstrap', async (route) => {
    const response = await route.fetch();
    const snapshot = await response.json();
    snapshot.changes_state = { data: state };
    const headers = { ...response.headers() };
    delete headers.etag;
    await route.fulfill({ response, headers, json: snapshot });
  });
  return { reads: () => reads };
}

export async function minThreadPane(page: Page): Promise<number> {
  const row = await page.evaluate((selector) => {
    const root = getComputedStyle(document.documentElement);
    return {
      remPx: parseFloat(root.fontSize),
      reservePx: parseFloat(root.getPropertyValue('--titlebar-lights-reserve')),
      leadControls: document.querySelectorAll(selector).length,
    };
  }, THREAD_ROW_LEAD_CONTROLS);
  return computeMinThreadPaneWidth(row.remPx, row.reservePx, row.leadControls);
}

export async function renderWholeTranscript(page: Page): Promise<void> {
  const chevron = page.locator('.scroll-to-top.visible').first();
  if (await chevron.count() === 0) return;
  await chevron.click();
  await expect(chevron).toBeHidden({ timeout: 30_000 });
  await expect.poll(() => page.evaluate(() => new Promise<boolean>(resolve => {
    const el = document.querySelector('.thread-content') as HTMLElement | null;
    if (!el) return resolve(false);
    const first = el.scrollTop;
    requestAnimationFrame(() => requestAnimationFrame(() => {
      resolve(first === 0 && el.scrollTop === 0);
    }));
  }))).toBe(true);
}

export async function waitForEventStream(page: Page, timeout = 10_000): Promise<void> {
  await page.waitForFunction(() => {
    return document.documentElement.dataset.lucidosEventStream === 'connected';
  }, undefined, { timeout });
}

export function watchPreferenceReads(page: Page): () => Promise<void> {
  let documentSeq = 0;
  let answered = 0;
  const pending = new Map<Request, number>();
  const inMainFrame = (r: Request) => !r.serviceWorker() && r.frame() === page.mainFrame();
  const isShellRead = (r: Request) => r.method() === 'GET'
    && inMainFrame(r)
    && new URL(r.url()).pathname.endsWith('/api/v1/preferences');
  page.on('requestfinished', async (r) => {
    const seq = pending.get(r);
    if (seq === undefined) return;
    pending.delete(r);
    if (seq === documentSeq && (await r.response())?.ok()) answered++;
  });
  page.on('requestfailed', (r) => { pending.delete(r); });
  page.on('request', (r) => {
    if (r.isNavigationRequest() && inMainFrame(r)) {
      documentSeq++;
      answered = 0;
    } else if (isShellRead(r)) {
      pending.set(r, documentSeq);
    }
  });
  return async () => {
    await waitForEventStream(page);
    await expect
      .poll(() => answered >= 2 && [...pending.values()].every(seq => seq !== documentSeq), {
        timeout: 10_000,
        message: 'the shell\'s startup preference reads never all landed',
      })
      .toBe(true);
    await page.evaluate(() => new Promise<void>(resolve => {
      requestAnimationFrame(() => requestAnimationFrame(() => resolve()));
    }));
  };
}

export async function sendMessage(page: Page, text: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(text);
  if (isMobileViewport(page)) {
    await clickVisibleElement(page, 'button[aria-label="Send message"]');
  } else {
    await input.press('Enter');
  }
}

export async function sendMessageCapturingThreadId(page: Page, text: string): Promise<string> {
  const posted = page.waitForRequest(
    (req) => req.url().includes('/api/v1/chat/stream') && req.method() === 'POST',
    { timeout: 60_000 },
  );
  await sendMessage(page, text);
  const threadId = (await posted).postDataJSON()?.thread_id;
  expect(threadId, 'chat POST carries the thread id').toBeTruthy();
  return threadId as string;
}

const TURN_RESPONSE_SELECTOR = '.response-content:not(.welcome-message)';

export async function waitForResponse(page: Page, timeout = 90_000): Promise<Locator> {
  await page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, TURN_RESPONSE_SELECTOR, { timeout });

  const allResponses = page.locator(TURN_RESPONSE_SELECTOR);
  const count = await allResponses.count();
  let response = allResponses.first();
  for (let i = 0; i < count; i++) {
    if (await allResponses.nth(i).isVisible().catch(() => false)) {
      response = allResponses.nth(i);
      break;
    }
  }

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

export async function revealSteps(page: Page, timeout = 30_000): Promise<void> {
  const toggle = page
    .locator('.turn-controls [data-role="toggle-steps"]:visible')
    .first();
  await expect(toggle).toBeVisible({ timeout });
  if (await toggle.getAttribute('aria-pressed') === 'false') await toggle.click();
  await expect(toggle).toHaveAttribute('aria-pressed', 'true');
}

export async function waitForVisibleElement(page: Page, selector: string, timeout = 5_000): Promise<void> {
  await page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => el.getBoundingClientRect().width > 0);
  }, selector, { timeout });
}

export async function waitAndClick(page: Page, selector: string, text?: string, timeout = 5_000): Promise<void> {
  await waitForVisibleElement(page, selector, timeout);
  await clickVisibleElement(page, selector, text);
}

async function pickerStepSettled(row: Locator): Promise<void> {
  await expect
    .poll(() => row.evaluate(el => el.closest('[data-overlay-panel]')?.getAnimations({ subtree: true }).length ?? 0),
      { timeout: 5_000, message: 'the picker step never finished gliding in' })
    .toBe(0);
}

export async function pickModelPair(page: Page, model: string, tier?: string): Promise<string> {
  const modelRow = page.locator(`.control-option[data-value="${model}"]:visible`).first();
  await expect(modelRow).toBeVisible({ timeout: 10_000 });
  await pickerStepSettled(modelRow);
  if ((await modelRow.locator('.control-option-more').count()) === 0) {
    if (tier) throw new Error(`pickModelPair: "${model}" offers no tiers, so "${tier}" cannot be picked`);
    await modelRow.click();
    await expect(page.locator('.control-option:visible')).toHaveCount(0);
    return `${model}|`;
  }
  await modelRow.click();
  const row = tier
    ? page.locator(`.control-option[data-value="${model}|${tier}"]:visible`).first()
    : page.locator(`.control-option[data-value^="${model}|"]:visible`).first();
  await expect(row).toBeVisible({ timeout: 5_000 });
  await pickerStepSettled(row);
  const pair = (await row.getAttribute('data-value')) ?? '';
  await row.click();
  return pair;
}

export interface RungRow {
  value: string;
  model: string;
  effort: string | null;
  current: boolean;
}

export function splitRungValue(value: string): { model: string; effort: string | null } {
  const at = value.lastIndexOf('|');
  if (at === -1) return { model: value, effort: null };
  return { model: value.slice(0, at), effort: value.slice(at + 1) || null };
}

const RUNG_PICKABLE = '.control-dropdown .control-option:visible:not([data-value="ladder-link"])';

export async function openRungList(page: Page): Promise<Locator> {
  const trigger = page.locator('.rung-trigger[aria-label^="Model"]:visible').first();
  const more = page.locator('.prompt-actions-row:visible [data-fold-key=""]').first();
  const foldedRow = page.locator('.thread-overflow-item[data-role="rung-control"]:visible');
  await expect(async () => {
    if (await trigger.isVisible()) {
      await trigger.click();
      return;
    }
    await more.click();
    try {
      await expect(foldedRow).toBeVisible({ timeout: 1_000 });
    } catch (err) {
      await page.keyboard.press('Escape');
      throw err;
    }
    await foldedRow.click();
  }).toPass({ timeout: 30_000 });
  const panel = page.locator('.control-dropdown:visible').first();
  await expect(panel.locator('.control-option:visible').first()).toBeVisible({ timeout: 10_000 });
  return panel;
}

export async function readRungRows(page: Page): Promise<RungRow[]> {
  await expect(page.locator(RUNG_PICKABLE).first()).toBeVisible({ timeout: 5_000 });
  const rows = await page.locator(RUNG_PICKABLE).evaluateAll((els) =>
    els.map((el) => ({
      value: el.getAttribute('data-value') ?? '',
      current: el.classList.contains('control-option-current'),
    })));
  return rows.map((r) => ({ ...r, ...splitRungValue(r.value) }));
}

export async function currentRungValue(page: Page): Promise<string> {
  const current = page.locator('.control-dropdown .control-option-current:visible');
  await expect(current).toHaveCount(1, { timeout: 5_000 });
  return (await current.getAttribute('data-value')) ?? '';
}

export function modelSubstitutions(threadId: string): { requested: string; served: string }[] {
  const rows = psql(
    `SELECT payload FROM events WHERE thread_id = '${threadId}' ` +
    `AND event_type = 'CodingAgentModelSubstituted' ORDER BY id`,
  );
  return rows.split('\n').filter(Boolean).map((row) => JSON.parse(row));
}

export async function expectPickRanOrSubstituted(page: Page, threadId: string, picked: RungRow): Promise<void> {
  await openRungList(page);
  const current = await currentRungValue(page);
  await page.keyboard.press('Escape');
  const substitutions = modelSubstitutions(threadId);
  if (substitutions.length === 0) {
    expect(current).toBe(picked.value);
    return;
  }
  expect(substitutions).toHaveLength(1);
  expect(substitutions[0].requested).toBe(picked.model);
  expect(splitRungValue(current).model).not.toBe(picked.model);
  expect(splitRungValue(current).effort).toBeTruthy();
  await expect(page.locator('.step-description', { hasText: 'this account cannot use' })).toHaveCount(1);
}

export async function pickRung(page: Page, value: string): Promise<void> {
  const row = page.locator(`.control-dropdown .control-option[data-value="${value}"]:visible`).first();
  await expect(row).toBeVisible({ timeout: 5_000 });
  await pickerStepSettled(row);
  await row.click();
  await expect(page.locator('.control-dropdown:visible')).toHaveCount(0, { timeout: 5_000 });
  await answerRungSwitchIfAsked(page, 2_000);
}

export async function answerRungSwitchIfAsked(page: Page, timeout: number): Promise<void> {
  const confirm = page.locator('.confirm-dialog');
  const asked = await confirm.waitFor({ state: 'visible', timeout }).then(() => true, () => false);
  if (!asked) return;
  const { switchHereLabel, switchLabel } = rungSwitchCopy({ from: '', to: '', extraCostUsd: null, canHandOff: true });
  const switchThread = confirm.getByRole('button', { name: switchHereLabel })
    .or(confirm.getByRole('button', { name: switchLabel, exact: true }));
  await switchThread.click();
  await expect(confirm).toHaveCount(0, { timeout: 5_000 });
}

export async function clickHeaderAction(page: Page, actionSelector: string, timeout = 10_000): Promise<void> {
  const menuRow = `.thread-overflow-item${actionSelector}`;
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

export async function headerActionOffered(page: Page, actionSelector: string, timeout = 10_000): Promise<boolean> {
  await page.waitForFunction(({ sel }) => {
    const anyVisible = (s: string) =>
      Array.from(document.querySelectorAll(s)).some(el => el.getBoundingClientRect().width > 0);
    return anyVisible(sel) || anyVisible('.content-header-more');
  }, { sel: actionSelector }, { timeout }).catch(() => {
  });

  if (await page.locator(`${actionSelector}:visible`).count() > 0) return true;
  if (!await clickVisibleElement(page, '.content-header-more')) return false;
  await waitForVisibleElement(page, '.thread-overflow-item', timeout);
  const offered = await page.locator(`.thread-overflow-item${actionSelector}`).count() > 0;
  await clickVisibleElement(page, '.content-header-more');
  await page.waitForFunction(
    () => document.querySelectorAll('.thread-overflow-item').length === 0,
    undefined, { timeout },
  );
  return offered;
}

export async function focusedThreadId(page: Page): Promise<string> {
  const id = await page.evaluate(() => localStorage.getItem('lucidos-focused-thread'));
  if (!id) throw new Error('focusedThreadId: no thread is focused (lucidos-focused-thread is unset)');
  return id;
}

export function threadRowFor(threadId: string): string {
  return `[data-flip-id="${threadId}"] .thread-row`;
}

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

export async function showGrouping(page: Page, grouping: 'folders' | 'ongoing'): Promise<void> {
  const label = grouping === 'ongoing' ? 'Show Ongoing' : 'Show Folders';
  const other = grouping === 'ongoing' ? 'Show Folders' : 'Show Ongoing';
  const shown = await page.locator(`.grouping-btn:visible[aria-label^="${other}"]`).count();
  if (shown > 0) return;
  const switched = await clickVisibleElement(page, `.grouping-btn[aria-label^="${label}"]`);
  if (!switched) throw new Error('The threads header grouping button is not visible');
  await expect(page.locator(`.grouping-btn:visible[aria-label^="${other}"]`)).toHaveCount(1);
}

export async function showOngoingGroup(page: Page, label: string): Promise<void> {
  await showGrouping(page, 'ongoing');
  const tile = page.locator('.drawer-ongoing-tiles .ongoing-tile:visible')
    .filter({ has: page.locator('.ongoing-tile-label', { hasText: new RegExp(`^${label}$`) }) });
  await expect(tile).toHaveCount(1);
  if (await tile.getAttribute('aria-current') !== 'true') await tile.click();
  await expect(tile).toHaveAttribute('aria-current', 'true');
}

export async function newThread(page: Page): Promise<void> {
  const clicked = await clickVisibleElement(page, 'button[aria-label="New thread"]');
  if (!clicked && isMobileViewport(page)
    && await clickVisibleElement(page, 'button[aria-label^="Lucidos menu"]')) {
    const row = page.locator('.brand-menu-item:visible', { hasText: 'New thread' });
    try {
      await row.waitFor({ state: 'visible', timeout: 5_000 });
    } catch (err) {
      await page.keyboard.press('Escape');
      throw new Error(`the Lucidos menu never showed its New thread row: ${(err as Error).message}`);
    }
    if (!await clickVisibleElement(page, '.brand-menu-item', 'New thread')) {
      throw new Error('the Lucidos menu New thread row vanished before it could be clicked');
    }
  }
  await ensureOnThreadPane(page);
  await page.waitForFunction((sel) => {
    return !Array.from(document.querySelectorAll(sel)).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, USER_MSG_SELECTOR, { timeout: 5_000 });
  await waitForVisibleInput(page, 5_000);
}

export async function openThreadDrawer(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    await ensureMobileView(page, 'threads');
    await page.waitForFunction(() => {
      const drawer = document.querySelector('.mobile-threads-pane .thread-drawer');
      return drawer && drawer.getBoundingClientRect().width > 0;
    }, undefined, { timeout: 5_000 });
    await waitForPaneAtRest(page);
    return;
  }
  const drawerShown = () => {
    const drawers = document.querySelectorAll('.thread-drawer:not(.thread-drawer-collapsed)');
    return Array.from(drawers).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  };
  const restoredOpen = await page.evaluate(
    () => localStorage.getItem('lucidos-thread-drawer-open') === 'true',
  );
  const isOpen = restoredOpen
    ? await page.waitForFunction(drawerShown, undefined, { timeout: 5_000 }).then(() => true, () => false)
    : await page.evaluate(drawerShown);
  if (!isOpen) {
    await page.locator(`.thread-toggle-slot button[aria-label^="${DRAWER_TOGGLE_LABEL}"]`).click();
  }
  const token = ++settleToken;
  await page.waitForFunction((token) => {
    const drawer = Array.from(document.querySelectorAll('.thread-drawer:not(.thread-drawer-collapsed)'))
      .find(el => {
        const r = el.getBoundingClientRect();
        return r.width > 0 && r.height > 0;
      });
    if (!drawer) return false;
    if (drawer.getAnimations().some(a => a.playState === 'running')) return false;
    const w = drawer.getBoundingClientRect().width;
    const win = window as unknown as { __luDrawerSettle?: { token: number; w: number } };
    const prev = win.__luDrawerSettle;
    win.__luDrawerSettle = { token, w };
    return w > 100 && prev?.token === token && prev.w === w;
  }, token, { timeout: 5_000 });
}

export function uniqueMessage(prefix = 'e2e-test'): string {
  return `${prefix}-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
}

export async function blurActiveElement(page: Page): Promise<void> {
  await page.evaluate(() => {
    if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
  });
}

export async function stampOverlayBuild(page: Page): Promise<void> {
  await page.evaluate(() => document.documentElement.setAttribute('data-titlebar-overlay', ''));
}

export async function getHeaderTop(page: Page): Promise<number> {
  return page.evaluate(() => {
    const header = document.querySelector('.app-header');
    return header ? header.getBoundingClientRect().top : -999;
  });
}

export async function enableMobileDynamicBars(page: Page): Promise<void> {
  const res = await apiRequest(page).put('/api/v1/preferences?key=mobile_dynamic_bars', {
    data: { value: 'true' },
  });
  expect(res.ok()).toBeTruthy();
}

export async function disableMobileDynamicBars(page: Page): Promise<void> {
  const res = await apiRequest(page).put('/api/v1/preferences?key=mobile_dynamic_bars', {
    data: { value: 'false' },
  });
  expect(res.ok()).toBeTruthy();
}

export async function setVoiceEnabled(page: Page, on: boolean): Promise<void> {
  const res = await apiRequest(page).put('/api/v1/preferences?key=voice_enabled', {
    data: { value: String(on) },
  });
  expect(res.ok()).toBeTruthy();
}

export const COMPOSER_FOLD_TRIGGER = '.prompt-actions-row .prompt-actions-more:visible';

export async function composerControl(page: Page, selector: string): Promise<Locator> {
  const inRow = page.locator(`.prompt-actions-row ${selector}:visible`).first();
  if (await inRow.count() > 0) return inRow;
  const more = page.locator(COMPOSER_FOLD_TRIGGER).first();
  if (await more.count() === 0) {
    throw new Error(`no ${selector} in the composer row, and no ⋯ trigger to look behind`);
  }
  await more.click();
  return page.locator(`.thread-overflow-menu ${selector}`).first();
}

export async function closeComposerFold(page: Page): Promise<void> {
  const open = page.locator(`${COMPOSER_FOLD_TRIGGER}[aria-expanded="true"]`);
  if (await open.count() > 0) await page.keyboard.press('Escape');
}

async function readComposerControl<T>(
  page: Page, selector: string, read: (control: Locator) => Promise<T>,
): Promise<T> {
  await closeComposerFold(page);
  try {
    return await read(await composerControl(page, selector));
  } finally {
    await closeComposerFold(page);
  }
}

export function codingAgentCommandsReady(page: Page): Promise<boolean> {
  return readComposerControl(page, '.commands-btn',
    (control) => control.evaluate((el) => el.classList.contains('commands-btn-active')));
}

export function composerToggleState(page: Page, selector: string): Promise<string | null> {
  return readComposerControl(page, selector,
    async (toggle) => {
      const quick = { timeout: 2_000 };
      return await toggle.getAttribute('aria-pressed', quick) ?? await toggle.getAttribute('aria-checked', quick);
    });
}

export async function pressComposerControl(page: Page, selector: string): Promise<void> {
  await expect(async () => {
    await closeComposerFold(page);
    await (await composerControl(page, selector)).click({ timeout: 2_000 });
  }).toPass({ timeout: 15_000 });
}

export function followToggle(page: Page): Locator {
  return page.locator('.prompt-area > [data-role="follow-live-edge"]:visible').first();
}

export function followToggleState(page: Page): Promise<string | null> {
  return followToggle(page).getAttribute('aria-pressed', { timeout: 2_000 });
}

export async function pressFollowToggle(page: Page): Promise<void> {
  await followToggle(page).click();
}

export async function assertHealthy(page: Page): Promise<void> {
  const response = await page.request.get('/api/v1/health');
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body.status).toBe('ok');
}

const MENU_OPTION = '.dropdown-menu .dropdown-option';

export async function pickDropdownOption(page: Page, rootSelector: string, optionLabel: string): Promise<void> {
  const opened = await clickVisibleElement(page, `${rootSelector} .dropdown-trigger`);
  if (!opened) throw new Error(`pickDropdownOption: no visible ${rootSelector} .dropdown-trigger`);
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

export async function ensureSlashCommandsReported(page: Page): Promise<void> {
  const reported = async () => {
    const res = await page.request.get('/api/v1/claude-code/commands?repo_id=');
    return ((await res.json()).builtin_commands ?? []).length;
  };
  if (await reported() > 0) return;
  await navigateToApp(page);
  await newThread(page);
  await pickComposeDestination(page);
  await sendMessage(page, 'Say exactly: "seed" and nothing else. Do not create any files.');
  await waitForActionPanel(page, 'Archive', 120_000);
  await expect.poll(reported).toBeGreaterThan(0);
}

export async function pickComposeDestination(page: Page, optionLabel = 'Lucidos source'): Promise<void> {
  await ensureOnThreadPane(page);
  await pickDropdownOption(page, '.compose-destination-picker', optionLabel);
}

export async function sendFollowUp(page: Page, text: string): Promise<void> {
  const input = await waitForVisibleInput(page, 15_000);
  await input.fill(text);
  if (isMobileViewport(page)) {
    await clickVisibleElement(page, 'button[aria-label="Send message"]');
  } else {
    await input.press('Enter');
  }
}

export async function countExchanges(page: Page): Promise<number> {
  return page.evaluate(() => {
    const exchanges = document.querySelectorAll('.chat-exchange');
    return Array.from(exchanges).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length;
  });
}

export async function waitForExchangeCount(page: Page, minCount: number, timeout = 30_000): Promise<void> {
  await page.waitForFunction((min) => {
    const exchanges = document.querySelectorAll('.chat-exchange');
    return Array.from(exchanges).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length >= min;
  }, minCount, { timeout });
}

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

export async function clickChangeAction(
  page: Page,
  label: 'Apply' | 'Discard' | 'Diff' | 'Archive',
  timeout = 15_000,
): Promise<void> {
  const direct = page.locator(
    `.thread-action-buttons:visible button.action-btn:not(.split-button-whole):has-text("${label}")`,
  ).first();
  const opener = page.locator(
    '.thread-action-buttons:visible .split-button-caret, .thread-action-buttons:visible .split-button-whole',
  ).first();
  await expect(direct.or(opener)).toBeVisible({ timeout });
  if (await direct.isVisible()) {
    await direct.click();
    return;
  }
  await opener.click();
  await page.locator(`.split-button-menu:visible button:has-text("${label}")`).first().click();
}

export async function waitForCCToFinish(page: Page, timeout = 120_000): Promise<void> {
  await page.waitForFunction(() => {
    const shown = (el: Element) => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    };
    if (!Array.from(document.querySelectorAll('.response-panel')).some(shown)) return false;
    const visible = Array.from(document.querySelectorAll('.response-panel .exchange-status-label')).filter(shown);
    if (visible.length === 0) return true;
    const last = visible[visible.length - 1];
    const text = last.textContent ?? '';
    return !(text.includes('Working') || text.includes('Requesting'));
  }, undefined, { timeout });
}

export async function waitForStreamingToStart(page: Page, minLength = 5, timeout = 30_000): Promise<void> {
  await page.waitForFunction(({ sel, min }) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').length > min;
    });
  }, { sel: TURN_RESPONSE_SELECTOR, min: minLength }, { timeout });
}

export async function waitForKeptOutputToStart(page: Page, minLength = 1, timeout = 30_000): Promise<void> {
  await page.waitForFunction(({ sel, min }) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      const settled = el.cloneNode(true) as HTMLElement;
      settled.querySelectorAll('[data-role="inline-step"]').forEach((step) => {
        const name = step.querySelector('.step-description')?.textContent?.trim();
        if (name === 'Thinking') step.remove();
      });
      return (settled.textContent ?? '').length > min;
    });
  }, { sel: TURN_RESPONSE_SELECTOR, min: minLength }, { timeout });
}

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

export async function assertUserMessagesInOrder(page: Page, markers: string[], timeout = 15_000): Promise<void> {
  await expect(async () => {
    const positions = await page.evaluate(({ sel, ms }) => {
      const visibleTexts = Array.from(document.querySelectorAll(sel))
        .filter(el => {
          const rect = el.getBoundingClientRect();
          return rect.width > 0 && rect.height > 0;
        })
        .map(el => el.textContent ?? '');
      return ms.map(m => visibleTexts.findIndex(t => t.includes(m)));
    }, { sel: USER_MSG_SELECTOR, ms: markers });
    expect(positions, `User messages not visible: ${markers.join(', ')}`).not.toContain(-1);
    expect(positions, `User messages out of sent order: ${markers.join(', ')}`)
      .toEqual([...positions].sort((a, b) => a - b));
  }).toPass({ timeout });
}

export async function countVisibleResponses(page: Page): Promise<number> {
  return page.evaluate((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    }).length;
  }, TURN_RESPONSE_SELECTOR);
}

export async function waitForVisibleResponseCount(
  page: Page,
  count: number,
  timeout = 90_000,
): Promise<void> {
  await page.waitForFunction(({ sel, n }) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    }).length >= n;
  }, { sel: TURN_RESPONSE_SELECTOR, n: count }, { timeout });
}

export async function getLatestVisibleResponseText(page: Page): Promise<string> {
  return page.evaluate((sel) => {
    const els = document.querySelectorAll(sel);
    const visible = Array.from(els).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    });
    return (visible[visible.length - 1]?.textContent ?? '').trim();
  }, TURN_RESPONSE_SELECTOR);
}

export async function countVisibleThreadRows(page: Page): Promise<number> {
  return page.evaluate(() => {
    const rows = document.querySelectorAll('.thread-row');
    return Array.from(rows).filter(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    }).length;
  });
}

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

export async function openFilesPanel(page: Page): Promise<void> {
  if (isMobileViewport(page)) {
    await ensureMobileView(page, 'content');
    await clickVisibleElement(page, '.hamburger-panel');
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

export function addTriggerCard(page: Page): Locator {
  return page.locator('.list-row-add-card:visible', { hasText: 'Add Trigger' }).first();
}

export async function openTriggersPanel(page: Page): Promise<void> {
  await ensureMobileView(page, 'content');
  await clickVisibleElement(page, '.hamburger-panel');
  await page.waitForFunction(() => {
    const items = document.querySelectorAll('.drawer-item');
    return Array.from(items).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 5_000 });
  await clickVisibleElement(page, '.drawer-item', 'Triggers');
  await expect(addTriggerCard(page)).toBeVisible({ timeout: 10_000 });
}

export async function dismissCCSession(page: Page): Promise<void> {
  try {
    await ensureOnThreadPane(page);
    await clickVisibleElement(page, '.thread-action-buttons button.action-btn', 'Archive');
  } catch {
  }
}

export const VISIBLE_TITLE_SELECTOR = '.thread-title';

export async function waitForThreadTitle(page: Page, timeout = 30_000): Promise<void> {
  await page.waitForFunction((sel) => {
    const els = document.querySelectorAll(sel);
    return Array.from(els).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').trim().length > 0;
    });
  }, VISIBLE_TITLE_SELECTOR, { timeout });
}

function focusedPinSelector(label: string): string {
  return `.thread-view-header-actions button[aria-label="${label}"]:visible`;
}

const MOBILE_TITLE_MENU = '.mobile-thread-title-row .thread-title-menu:visible';

function isPhoneLayout(page: Page): boolean {
  return (page.viewportSize()?.width ?? Infinity) <= 768;
}

function threadMenuItem(page: Page, label: string): Locator {
  return page.locator('.thread-overflow-menu').getByRole('menuitem', { name: label, exact: true });
}

export async function toggleFocusedPin(page: Page, to: 'pinned' | 'unpinned'): Promise<void> {
  if (!isPhoneLayout(page)) {
    await page.locator(focusedPinSelector(to === 'pinned' ? 'Pin thread' : 'Remove thread from Pinned section')).first().click();
    return;
  }
  await page.locator(MOBILE_TITLE_MENU).click();
  await threadMenuItem(page, to === 'pinned' ? 'Pin thread' : 'Unpin thread').click();
}

export async function expectFocusedPinned(page: Page, pinned: boolean, timeout = 10_000): Promise<void> {
  if (!isPhoneLayout(page)) {
    await expect(page.locator(focusedPinSelector(pinned ? 'Remove thread from Pinned section' : 'Pin thread')).first())
      .toBeVisible({ timeout });
    return;
  }
  await expect(async () => {
    await page.locator(MOBILE_TITLE_MENU).click();
    try {
      await expect(threadMenuItem(page, pinned ? 'Unpin thread' : 'Pin thread')).toBeVisible({ timeout: 1_000 });
    } finally {
      await page.keyboard.press('Escape');
      await expect(page.locator('.thread-overflow-menu')).toHaveCount(0);
    }
  }).toPass({ timeout });
}

export async function renameThreadViaMenu(page: Page, title: string): Promise<void> {
  const opened = await clickVisibleElement(page, '.thread-title-menu, .thread-row-focused [aria-label="More thread actions"]');
  if (!opened) throw new Error('no visible thread menu to rename from');
  await page.locator('.thread-overflow-menu [role="menuitem"]', { hasText: 'Rename' }).first().click();
  const field = page.locator('.confirm-dialog .prompt-input');
  await field.waitFor({ state: 'visible' });
  await field.fill(title);
  await page.locator('[data-role="prompt-ok"]').click();
}

export async function getMobileTitleHeight(page: Page): Promise<number> {
  return page.evaluate(() => {
    const els = document.querySelectorAll('.mobile-thread-title-row .thread-title');
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0) return rect.height;
    }
    return 0;
  });
}

export async function getVisibleTitleText(page: Page): Promise<string> {
  return page.evaluate((sel) => {
    const els = document.querySelectorAll(sel);
    for (const el of els) {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) {
        return (el.textContent ?? '').trim();
      }
    }
    return '';
  }, VISIBLE_TITLE_SELECTOR);
}


export interface PushLogEntry {
  device_id: string;
  notification_id: string;
  sent_at: string;
  payload?: string | null;
}

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

export async function waitForScrollSettled(page: Page): Promise<void> {
  const read = () => page.evaluate(() => {
    const el = Array.from(document.querySelectorAll<HTMLElement>('.thread-content'))
      .find(c => c.getBoundingClientRect().height > 0);
    return el ? `${Math.round(el.scrollTop)}:${Math.round(el.scrollHeight)}` : 'gone';
  });
  let previous = await read();
  let stable = 0;
  await expect.poll(async () => {
    const now = await read();
    stable = now === previous ? stable + 1 : 0;
    previous = now;
    return stable;
  }, { timeout: 15_000, intervals: [50, 50, 100] }).toBeGreaterThanOrEqual(2);
}

export async function rightClickInsideFrame(page: Page, frame: Locator): Promise<Locator> {
  const b = (await frame.boundingBox())!;
  const at = { x: b.x + Math.min(40, b.width / 2), y: b.y + Math.min(30, b.height / 2) };
  await page.mouse.click(at.x, at.y, { button: 'right' });
  const menu = page.locator('.thread-overflow-menu');
  await expect(menu).toHaveCount(1);
  await expect(menu).toBeVisible();
  const m = (await menu.boundingBox())!;
  const edgeGap = Math.min(Math.abs(m.y - at.y), Math.abs(m.y + m.height - at.y));
  expect(Math.abs(edgeGap - ANCHOR_GAP)).toBeLessThanOrEqual(1);
  expect(at.x).toBeGreaterThanOrEqual(m.x - 2);
  expect(at.x).toBeLessThanOrEqual(m.x + m.width + 2);
  return menu;
}

export async function openWidgetCardMenu(page: Page, card: Locator): Promise<Locator> {
  await card.focus();
  await page.keyboard.press('Shift+F10');
  const menu = page.locator('.thread-overflow-menu');
  await expect(menu).toBeVisible();
  return menu;
}
