/**
 * The floating scroll chevrons take a tap that lands NEAR them. Both sit on
 * the pane's right edge, and so does every step row's context counter. Without
 * a reach, a tap just off the drawn circle lands on the counter underneath,
 * which opens the context viewer instead of scrolling.
 *
 * On touch each chevron carries a transparent `::before` that reaches past its
 * circle (chat/input-messages.css, `--scroll-chevron-reach`). The circle and
 * the glyph stay where they were. A hidden chevron's reach takes no taps.
 */
import { randomUUID } from 'crypto';
import { test, expect, Page } from './fixtures';
import { assertHealthy, disarmFollowSeed, ensureMobileView, navigateToApp, revealSteps } from './helpers';
import { psql } from './db-helpers';

test.use({ viewport: { width: 393, height: 852 } });

const TURNS = 5;
const STEPS_PER_TURN = 14;

/** A chat thread whose every step row carries a context counter. A legacy
 *  `ThoughtStreamed` with `context_tokens` is the smallest payload that gives a
 *  step a snapshot, which is what makes its counter a button. */
function seedThreadOfCounters(): string {
  const threadId = randomUUID();
  const base = Date.now();
  let n = 0;
  const at = () => new Date(base + n++ * 1000).toISOString();
  const rows: string[] = [];
  const row = (type: string, payload: string) =>
    `('${randomUUID()}', '${type}', '${payload}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`;

  for (let t = 0; t < TURNS; t++) {
    const messageId = randomUUID();
    rows.push(`('${messageId}', 'MessageReceived', '{"text":"turn ${t}","mode":"human","channel":"chat"}'::jsonb, '${at()}', 'thread', '${threadId}', '${threadId}')`);
    for (let i = 0; i < STEPS_PER_TURN; i++) {
      const ref = `"request_event_id":"${messageId}"`;
      rows.push(
        row('ThoughtStreamed', `{"text":"","context_tokens":${40_000 + i * 1000},"context_messages":${i + 2},${ref}}`),
        row('ToolCalled', `{"name":"read_file","args":{"path":"notes/${t}-${i}.md"},${ref}}`),
        row('ToolResult', `{"name":"read_file","result":"ok",${ref}}`),
      );
    }
    rows.push(row('ResponseGenerated', `{"text":"Done ${t}.","images":[],"request_event_id":"${messageId}"}`));
  }

  psql([
    `INSERT INTO thread_summaries (thread_id, title, source, last_activity, message_count, is_saved, has_response, status, archive_state, state, is_coding_agent, active_children_count, total_children_count, coding_agent_proposed, coding_agent_requires_restart, coding_agent_is_external_repo) ` +
      `VALUES ('${threadId}', 'E2E scroll chevron reach', 'chat', '${new Date(base).toISOString()}', ${TURNS}, false, true, 'idle', 'archived', 'active', false, 0, 0, false, false, false)`,
    `INSERT INTO events (id, event_type, payload, created, aggregate, aggregate_id, thread_id) VALUES\n` + rows.join(',\n'),
  ].join(';\n'));
  return threadId;
}

const CHEVRONS = [
  { name: 'up', sel: '.mobile-swipe-pane .scroll-to-top' },
  { name: 'down', sel: '.mobile-swipe-pane .scroll-to-bottom' },
] as const;

interface Box { left: number; right: number; top: number; bottom: number }

interface Probe {
  box: Box;
  glyph: Box;
  /** The span around the circle's centre where a tap still reaches it. */
  hit: Box;
  /** What a tap N px off each side of the circle reaches, keyed `side+N`. */
  offsets: Record<string, string>;
  translate: string;
}

/** Name whatever a tap at (x, y) would reach. */
const DESCRIBE = `(x, y) => {
  const el = document.elementFromPoint(x, y);
  if (!el) return 'nothing';
  if (el.closest('.scroll-to-top')) return 'up chevron';
  if (el.closest('.scroll-to-bottom')) return 'down chevron';
  if (el.closest('[data-role="step-context"]')) return 'step-context';
  if (el.closest('[data-role="step-main"]')) return 'step-main';
  const control = el.closest('button, a, [role="button"]');
  if (control) return control.getAttribute('aria-label') || control.className;
  return 'bare ' + el.tagName.toLowerCase() + '.' + String(el.className).split(' ')[0];
}`;

async function probe(page: Page, sel: string, offsets: number[]): Promise<Probe | null> {
  return page.evaluate(({ sel, offsets, describeSrc }) => {
    const describe = new Function(`return ${describeSrc}`)() as (x: number, y: number) => string;
    const btn = document.querySelector(sel) as HTMLElement | null;
    const svg = btn?.querySelector('svg');
    if (!btn || !svg || btn.getBoundingClientRect().width === 0) return null;
    const rect = (el: Element) => {
      const r = el.getBoundingClientRect();
      return { left: r.left, right: r.right, top: r.top, bottom: r.bottom };
    };
    const box = rect(btn);
    const cx = (box.left + box.right) / 2;
    const cy = (box.top + box.bottom) / 2;
    const reaches = (x: number, y: number) => document.elementFromPoint(x, y)?.closest(sel) === btn;
    const walk = (dx: number, dy: number) => {
      let d = 0;
      while (d < 80 && reaches(cx + dx * (d + 0.5), cy + dy * (d + 0.5))) d += 0.5;
      return d;
    };
    const hit = { left: cx - walk(-1, 0), right: cx + walk(1, 0), top: cy - walk(0, -1), bottom: cy + walk(0, 1) };
    const out: Record<string, string> = {};
    for (const d of offsets) {
      out[`left+${d}`] = describe(box.left - d, cy);
      out[`right+${d}`] = describe(box.right + d, cy);
      out[`above+${d}`] = describe(cx, box.top - d);
      out[`below+${d}`] = describe(cx, box.bottom + d);
    }
    return { box, glyph: rect(svg), hit, offsets: out, translate: getComputedStyle(btn).translate };
  }, { sel, offsets, describeSrc: DESCRIBE });
}

/** Scroll the transcript so a step counter's centre sits level with the
 *  chevron's centre, which is the layout the report came from. */
async function parkCounterUnder(page: Page, sel: string): Promise<void> {
  const b = (await page.locator(sel).boundingBox())!;
  await parkCounterUnderPoint(page, b.y + b.height / 2);
}

/** Scroll so a step counter's centre sits at viewport height `y`. */
async function parkCounterUnderPoint(page: Page, y: number): Promise<void> {
  await page.evaluate((y) => {
    const scroller = document.querySelector('.mobile-swipe-pane .thread-content') as HTMLElement;
    const counters = [...document.querySelectorAll('.mobile-swipe-pane [data-role="step-context"]')] as HTMLElement[];
    const centre = (c: HTMLElement) => { const r = c.getBoundingClientRect(); return (r.top + r.bottom) / 2; };
    const best = counters.reduce((a, c) => (Math.abs(centre(c) - y) < Math.abs(centre(a) - y) ? c : a));
    scroller.scrollTop += centre(best) - y;
  }, y);
}

async function setScale(page: Page, scale: string, rootPx: number): Promise<void> {
  await page.evaluate((s) => document.documentElement.style.setProperty('--user-ui-scale', s), scale);
  await expect
    .poll(() => page.evaluate(() => getComputedStyle(document.documentElement).fontSize),
      { timeout: 5_000, message: `the root never settled at ui-scale ${scale}` })
    .toBe(`${rootPx}px`);
}

/** Open the seeded thread with both chevrons showing: the reader parked
 *  mid-transcript, neither at the top nor at the bottom. */
async function openMidThread(page: Page, threadId: string): Promise<void> {
  await disarmFollowSeed(page);
  await page.addInitScript((tid: string) => localStorage.setItem('lucidos-focused-thread', tid), threadId);
  await navigateToApp(page);
  await page.waitForFunction(() => localStorage.getItem('lucidos-ui-scale') !== null, undefined, { timeout: 10_000 });
  await ensureMobileView(page, 'thread');
  await revealSteps(page);
  await expect(page.locator('.mobile-swipe-pane [data-role="step-context"]').first()).toBeVisible({ timeout: 15_000 });
  await page.evaluate(() => {
    const el = document.querySelector('.mobile-swipe-pane .thread-content') as HTMLElement;
    el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
  });
  for (const c of CHEVRONS) await expect(page.locator(`${c.sel}.visible`)).toHaveCount(1);
}

const fmt = (b: Box) => `${(b.right - b.left).toFixed(1)}x${(b.bottom - b.top).toFixed(1)} at (${b.left.toFixed(1)}, ${b.top.toFixed(1)})`;

async function whatIsAt(page: Page, x: number, y: number): Promise<string> {
  return page.evaluate(({ x, y, describeSrc }) =>
    (new Function(`return ${describeSrc}`)() as (x: number, y: number) => string)(x, y),
  { x, y, describeSrc: DESCRIBE });
}

/** A box's size in px, resolved from a CSS length at the current root. */
async function lengthPx(page: Page, value: string): Promise<number> {
  return page.evaluate((value) => {
    const probe = document.createElement('div');
    probe.style.position = 'absolute';
    probe.style.width = value;
    document.body.appendChild(probe);
    const w = probe.getBoundingClientRect().width;
    probe.remove();
    return w;
  }, value);
}

const CONTEXT_MODAL = '[data-role="context-captured-modal"]';
const STEP_MODAL = '[data-role="step-detail-modal"]';
const SCALES = [['100%', 16], ['112.5%', 18]] as const;

/** The near misses a finger makes, in px at the 16px root, scaled with it. */
const NEAR_MISSES = [2, 4, 8];

test.describe('The floating scroll chevrons take a near miss', () => {
  const seeded: string[] = [];

  test.beforeEach(async ({ page }) => {
    await assertHealthy(page);
  });

  test.afterEach(() => {
    if (seeded.length === 0) return;
    const ids = seeded.splice(0).map(id => `'${id}'`).join(',');
    psql(`DELETE FROM events WHERE thread_id IN (${ids}); DELETE FROM thread_summaries WHERE thread_id IN (${ids})`);
  });

  test('each hit box is the full target, over a step row, with the circle unmoved', async ({ page }, info) => {
    const threadId = seedThreadOfCounters();
    seeded.push(threadId);
    await openMidThread(page, threadId);

    for (const [scale, rootPx] of SCALES) {
      await setScale(page, scale, rootPx);
      const circle = await lengthPx(page, 'var(--scroll-chevron-size)');
      const inset = await lengthPx(page, '1.125rem');
      const offsets = NEAR_MISSES.map(d => (d * rootPx) / 16);
      for (const c of CHEVRONS) {
        await parkCounterUnder(page, c.sel);
        await expect(page.locator(`${c.sel}.visible`)).toHaveCount(1);
        const p = (await probe(page, c.sel, [...offsets, (16 * rootPx) / 16]))!;
        const where = `${c.name} at ${scale}`;
        console.log(`[${info.project.name}] ${where}: visible ${fmt(p.box)}, glyph ${fmt(p.glyph)}, `
          + `hit ${fmt(p.hit)}, translate ${p.translate}\n  ${JSON.stringify(p.offsets)}`);

        // The circle kept its size and its inset from the pane's right edge,
        // with the glyph centred in it.
        expect(p.box.right - p.box.left, `${where}: the circle grew`).toBeCloseTo(circle, 0);
        expect(page.viewportSize()!.width - p.box.right, `${where}: the circle moved`).toBeCloseTo(inset, 0);
        expect((p.glyph.left + p.glyph.right) / 2, `${where}: the glyph moved`).toBeCloseTo((p.box.left + p.box.right) / 2, 0);

        // At least 44px at the 16px root on both axes. The half-px walk loses
        // up to a step at each end.
        const target = (44 * rootPx) / 16;
        expect(p.hit.right - p.hit.left, `${where}: hit width`).toBeGreaterThanOrEqual(target);
        expect(p.hit.bottom - p.hit.top, `${where}: hit height`).toBeGreaterThanOrEqual(target);

        for (const d of offsets) {
          for (const side of ['left', 'right', 'above', 'below']) {
            expect(p.offsets[`${side}+${d}`], `${where}: a tap ${d}px ${side} of the circle`).toBe(`${c.name} chevron`);
          }
        }
      }
    }
  });

  test('a real tap just off the circle scrolls, and opens no modal', async ({ page }) => {
    const threadId = seedThreadOfCounters();
    seeded.push(threadId);
    await openMidThread(page, threadId);

    for (const [scale, rootPx] of SCALES) {
      await setScale(page, scale, rootPx);
      const d = (6 * rootPx) / 16;
      for (const c of CHEVRONS) {
        for (const side of ['left', 'above', 'below'] as const) {
          await page.evaluate(() => {
            const el = document.querySelector('.mobile-swipe-pane .thread-content') as HTMLElement;
            el.scrollTop = (el.scrollHeight - el.clientHeight) / 2;
          });
          await expect(page.locator(`${c.sel}.visible`)).toHaveCount(1);
          const b = (await page.locator(c.sel).boundingBox())!;
          const cx = b.x + b.width / 2;
          const cy = b.y + b.height / 2;
          const [x, y] = side === 'left' ? [b.x - d, cy] : side === 'above' ? [cx, b.y - d] : [cx, b.y + b.height + d];
          const where = `${c.name} at ${scale}, tapped ${side}`;
          await parkCounterUnderPoint(page, y);
          expect((await page.locator(c.sel).boundingBox())!.y, `${where}: the chevron moved on the park`).toBeCloseTo(b.y, 0);
          // The step row is really there under the tap, not a turn boundary.
          // Straight above or below, that is the counter itself.
          const under = await page.evaluate(({ x, y, sel }) => {
            const btn = document.querySelector(sel) as HTMLElement;
            btn.style.visibility = 'hidden';
            const el = document.elementFromPoint(x, y);
            btn.style.visibility = '';
            return el?.closest('[data-role="step-context"]') ? 'step-context'
              : el?.closest('[data-role="inline-step"]') ? 'step row' : 'no step row';
          }, { x, y, sel: c.sel });
          expect(under, `${where}: what lies under the tap`).toBe(side === 'left' ? 'step row' : 'step-context');

          await page.touchscreen.tap(x, y);
          await expect(page.locator(`${c.sel}.visible`), `${where}: the chevron did not fire`).toHaveCount(0, { timeout: 5_000 });
          await expect(page.locator(CONTEXT_MODAL), `${where}: the context viewer opened`).toHaveCount(0);
          await expect(page.locator(STEP_MODAL), `${where}: the step detail opened`).toHaveCount(0);
        }
      }
    }
  });

  test('the counters stay tappable: away from the chevrons, and under a hidden one', async ({ page }) => {
    const threadId = seedThreadOfCounters();
    seeded.push(threadId);
    await openMidThread(page, threadId);
    const closeModal = async () => {
      await page.locator(`${CONTEXT_MODAL} [data-role="surface-close"]`).click();
      await expect(page.locator(CONTEXT_MODAL)).toHaveCount(0);
    };

    // A counter in the middle of the pane, clear of both chevrons.
    const mid = await page.evaluate(() => {
      const counters = [...document.querySelectorAll('.mobile-swipe-pane [data-role="step-context"]')] as HTMLElement[];
      const r = counters.map(c => c.getBoundingClientRect()).find(r => Math.abs((r.top + r.bottom) / 2 - innerHeight / 2) < 40)!;
      return { x: (r.left + r.right) / 2, y: (r.top + r.bottom) / 2 };
    });
    expect(await whatIsAt(page, mid.x, mid.y)).toBe('step-context');
    await page.touchscreen.tap(mid.x, mid.y);
    await expect(page.locator(CONTEXT_MODAL), 'a counter away from the chevrons opens the context viewer').toHaveCount(1);
    await closeModal();

    // A hidden chevron's reach goes with it. The bottom cannot scroll a
    // counter under the down chevron, so hide it where one already is, the
    // way ScrollControls does: by dropping `.visible`.
    const down = CHEVRONS[1].sel;
    const b = (await page.locator(down).boundingBox())!;
    const reachPoint = { x: b.x + b.width / 2, y: b.y - 6 };
    await parkCounterUnderPoint(page, reachPoint.y);
    expect(await whatIsAt(page, reachPoint.x, reachPoint.y), 'the reach point is not in the reach').toBe('down chevron');
    await page.evaluate((sel) => document.querySelector(sel)!.classList.remove('visible'), down);
    await expect.poll(() => whatIsAt(page, reachPoint.x, reachPoint.y), { message: 'a hidden chevron kept its reach' })
      .toBe('step-context');
    await page.touchscreen.tap(reachPoint.x, reachPoint.y);
    await expect(page.locator(CONTEXT_MODAL), 'the counter under a hidden chevron opens the context viewer').toHaveCount(1);
    await closeModal();

    // And the real thing: at the bottom the chevron is gone, reach included.
    await page.evaluate(() => {
      const el = document.querySelector('.mobile-swipe-pane .thread-content') as HTMLElement;
      el.scrollTop = el.scrollHeight;
    });
    await expect(page.locator(`${down}.visible`)).toHaveCount(0);
    await expect.poll(() => whatIsAt(page, reachPoint.x, reachPoint.y)).not.toMatch(/chevron/);
  });

  test('the reach leaves the title bar and the composer their taps', async ({ page }) => {
    const threadId = seedThreadOfCounters();
    seeded.push(threadId);
    await openMidThread(page, threadId);

    for (const [scale, rootPx] of SCALES) {
      await setScale(page, scale, rootPx);
      // A hidden chevron takes no tap at all, which would pass this vacuously.
      for (const c of CHEVRONS) await expect(page.locator(`${c.sel}.visible`)).toHaveCount(1);
      const up = (await page.locator(CHEVRONS[0].sel).boundingBox())!;
      const down = (await page.locator(CHEVRONS[1].sel).boundingBox())!;
      const [title, composer] = await page.evaluate(() => [
        document.querySelector('.mobile-swipe-pane .mobile-thread-title-row')!.getBoundingClientRect().bottom,
        document.querySelector('.mobile-swipe-pane .prompt-input-container')!.getBoundingClientRect().top,
      ]);
      expect(await whatIsAt(page, up.x + up.width / 2, title - 1), `the title bar at ${scale}`).not.toMatch(/chevron/);
      expect(await whatIsAt(page, down.x + down.width / 2, composer + 1), `the composer at ${scale}`).not.toMatch(/chevron/);
    }
  });

  test('the up chevron\'s reach follows it when the header hides', async ({ page }) => {
    const threadId = seedThreadOfCounters();
    seeded.push(threadId);
    await openMidThread(page, threadId);
    const up = CHEVRONS[0].sel;
    const resting = (await probe(page, up, []))!;

    // What useHideOnScroll does once the header is scrolled away: it slides
    // the header up by its own height and writes the same offset on its two
    // consumers, the title bar and the chevron. Written after the last scroll,
    // since the hook rewrites all three on every scroll frame.
    await page.evaluate((sel) => {
      const header = document.querySelector('.app-header') as HTMLElement;
      const height = header.getBoundingClientRect().height;
      header.style.translate = `0 ${-height}px`;
      for (const el of [document.querySelector(sel), document.querySelector('.mobile-swipe-pane .mobile-thread-title-row')]) {
        (el as HTMLElement).style.setProperty('--mobile-header-offset', `${-height}px`);
      }
    }, up);
    await expect.poll(async () => (await probe(page, up, []))!.box.top).toBeLessThan(resting.box.top - 10);
    const hidden = (await probe(page, up, NEAR_MISSES))!;
    expect(hidden.translate, 'the header offset never reached the chevron').not.toBe(resting.translate);
    const target = 44;
    expect(hidden.hit.right - hidden.hit.left).toBeGreaterThanOrEqual(target);
    expect(hidden.hit.bottom - hidden.hit.top).toBeGreaterThanOrEqual(target);
    // Centred on the moved circle, not left behind where it rested. Within
    // one step of the half-px walk.
    const drift = (hidden.hit.top + hidden.hit.bottom) / 2 - (hidden.box.top + hidden.box.bottom) / 2;
    expect(Math.abs(drift), 'the reach stayed behind the moved circle').toBeLessThanOrEqual(0.5);
    for (const [key, reached] of Object.entries(hidden.offsets)) {
      expect(reached, `with the header hidden, a tap ${key}`).toBe('up chevron');
    }
    // The title bar rode up with it and still keeps its own taps.
    const titleBottom = await page.evaluate(() =>
      document.querySelector('.mobile-swipe-pane .mobile-thread-title-row')!.getBoundingClientRect().bottom);
    expect(await whatIsAt(page, (hidden.box.left + hidden.box.right) / 2, titleBottom - 1)).not.toMatch(/chevron/);
  });
});
