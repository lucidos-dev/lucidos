import { Page, expect, Locator } from '@playwright/test';
import { readFileSync, existsSync } from 'fs';
import { resolve } from 'path';

const WORKSPACE = resolve(process.env.E2E_WORKSPACE ?? `${process.env.HOME}/workspaces/e2e-test`);

/** CSS selector for the body of a rendered user message (initiator panel).
 *  Centralized so a UI rename only requires changing this one constant. */
export const USER_MSG_SELECTOR = '.initiator-panel-user .initiator-body';

/** Locator for the first physically visible user-message body (dual-layout safe). */
export function userMessageBody(page: Page): Locator {
  return page.locator(`${USER_MSG_SELECTOR}:visible`).first();
}

/** Check if viewport is mobile-sized (matches CSS breakpoint at 768px) */
export function isMobileViewport(page: Page): boolean {
  const vp = page.viewportSize();
  return vp ? vp.width < 769 : false;
}

/** Navigate to a mobile pane by name. No-op on desktop or if already on the target pane.
 *  Re-clicks the dot inside the wait loop so a click absorbed by a concurrent
 *  re-render (e.g., dismiss-thread fan-out updating the drawer) gets retried.
 *  polling: 250 caps re-clicks at 4/sec instead of the rAF default (~60/sec)
 *  to avoid event-storming Preact handlers while the pane settles. */
async function ensureMobileView(page: Page, viewName: 'thread' | 'threads'): Promise<void> {
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
 *  At mobile viewports the desktop layout is display:none, so .first() may pick
 *  the hidden one — we wait for any prompt-input to become visible, then return
 *  the visible locator. */
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

export async function navigateToApp(page: Page): Promise<void> {
  await page.goto('/');
  await ensureOnThreadPane(page);
  await waitForVisibleInput(page);
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

/** Click compose button to start a new thread (dual-layout safe).
 *  On mobile the compose button navigates to thread pane automatically. */
export async function newThread(page: Page): Promise<void> {
  await clickVisibleElement(page, 'button[aria-label="New thread"]');
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
    await page.locator('button[aria-label="Toggle thread drawer"]').first().click();
    await page.waitForFunction(() => {
      const drawers = document.querySelectorAll('.thread-drawer:not(.thread-drawer-collapsed)');
      return Array.from(drawers).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0;
      });
    }, undefined, { timeout: 5_000 });
  }
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

export async function assertHealthy(page: Page): Promise<void> {
  const response = await page.request.get('/api/health');
  expect(response.ok()).toBeTruthy();
  const body = await response.json();
  expect(body.status).toBe('ok');
}

/** Switch the input mode to "Claude" (Claude Code) — dual-layout safe */
export async function switchToClaudeMode(page: Page): Promise<void> {
  await ensureOnThreadPane(page);
  const clicked = await clickVisibleElement(page, 'button.segmented-btn', 'Claude');
  if (clicked) {
    await page.waitForFunction(() => {
      const btns = document.querySelectorAll('button.segmented-btn.active');
      return Array.from(btns).some(el => {
        const rect = el.getBoundingClientRect();
        return rect.width > 0 && rect.height > 0 && (el.textContent ?? '').includes('Claude');
      });
    }, undefined, { timeout: 3_000 }).catch(() => {});
  }
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

/** Wait for CC to finish working (status clears or action panel appears) */
export async function waitForCCToFinish(page: Page, timeout = 120_000): Promise<void> {
  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    const hasWorking = Array.from(labels).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      const text = el.textContent ?? '';
      return text.includes('Working') || text.includes('Requesting');
    });
    if (!hasWorking) return true;
    const panels = document.querySelectorAll('.thread-action-buttons');
    return panels.length > 0;
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
    const cmdResp = await page.request.get(`/api/claude-code/commands?thread_id=${threadId}`);
    expect(cmdResp.ok()).toBeTruthy();
    cmdData = await cmdResp.json();
    expect(cmdData.has_active_session).toBe(true);
  }).toPass({ timeout, intervals: [500, 1000, 2000] });
  return cmdData;
}

/** Assert that all given markers appear in visible user-message body elements.
 *  Single page.evaluate scans the DOM once and returns the missing markers,
 *  giving an informative failure message instead of "expected false to be true". */
export async function assertUserMessagesVisible(page: Page, markers: string[]): Promise<void> {
  const missing = await page.evaluate(({ sel, ms }) => {
    const visibleTexts: string[] = [];
    document.querySelectorAll(sel).forEach(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width > 0 && rect.height > 0) visibleTexts.push(el.textContent ?? '');
    });
    return ms.filter(m => !visibleTexts.some(t => t.includes(m)));
  }, { sel: USER_MSG_SELECTOR, ms: markers });
  expect(missing, `User messages not visible: ${missing.join(', ')}`).toEqual([]);
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

/** Wait for stop button, click it, wait for Canceled status */
export async function cancelStreamingResponse(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const btns = document.querySelectorAll('.exchange-stop-btn');
    return Array.from(btns).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 30_000 });

  await clickVisibleElement(page, '.exchange-stop-btn');

  await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    return Array.from(labels).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      return (el.textContent ?? '').includes('Canceled');
    });
  }, undefined, { timeout: 15_000 });
}

/** Wait for stop button, click it (no confirm dialog for CC), wait for status change.
 *  Falls back to full cancel if interrupt doesn't take effect within 15s. */
export async function cancelCCResponse(page: Page): Promise<void> {
  await page.waitForFunction(() => {
    const btns = document.querySelectorAll('.exchange-stop-btn');
    return Array.from(btns).some(el => {
      const rect = el.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    });
  }, undefined, { timeout: 30_000 });

  await clickVisibleElement(page, '.exchange-stop-btn');

  // Wait up to 15s for interrupt to take effect
  const interruptWorked = await page.waitForFunction(() => {
    const labels = document.querySelectorAll('.exchange-status-label');
    const hasWorking = Array.from(labels).some(el => {
      const rect = el.getBoundingClientRect();
      if (rect.width === 0 || rect.height === 0) return false;
      const text = el.textContent ?? '';
      return text.includes('Working') || text.includes('Requesting');
    });
    return !hasWorking;
  }, undefined, { timeout: 15_000 }).then(() => true).catch(() => false);

  if (!interruptWorked) {
    // CC didn't respond to interrupt (may be stuck in a tool call).
    // Escalate: hit the cancel endpoint to kill the CC process.
    await page.request.post('/api/claude-code/cancel?discard=true').catch(() => {});

    // Wait for status to clear after cancel
    await page.waitForFunction(() => {
      const labels = document.querySelectorAll('.exchange-status-label');
      const hasWorking = Array.from(labels).some(el => {
        const rect = el.getBoundingClientRect();
        if (rect.width === 0 || rect.height === 0) return false;
        const text = el.textContent ?? '';
        return text.includes('Working') || text.includes('Requesting');
      });
      return !hasWorking;
    }, undefined, { timeout: 30_000 });
  }
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

/** Best-effort dismiss of an idle CC session by clicking Done (dual-layout safe) */
export async function dismissCCSession(page: Page): Promise<void> {
  try {
    await ensureOnThreadPane(page);
    await clickVisibleElement(page, '.thread-action-buttons button.action-btn', 'Done');
  } catch {
    // CC session may have already ended — not an error
  }
}
