/**
 * The *System attention badge* on the path in: the menu hamburger, the drawer's
 * Settings row, the Settings home's System row, and the System tab that owes
 * the work.
 *
 * The two hook-free components are invoked directly and their vnode trees
 * walked, the `thread-toggle-attention-badge.test.tsx` way. The other two hosts
 * are source-scanned. `Drawer` takes a hook, and `SettingsView` pulls in the
 * whole store. Standing either up would pin the mechanism rather than the
 * requirement, the reason `settings-nav-structure.test.ts` gives.
 *
 * What a scan can still hold there is the thing that actually goes wrong: the
 * mark landing on the wrong row, or on every row.
 */
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { describe, it, expect, beforeEach } from 'vitest';
import type { VNode } from 'preact';
import { SystemAttentionBadge } from '../SystemAttentionBadge';
import { HamburgerButton } from '../../layout/ContentNav';
import { findByClass, textOf, type AnyVNode } from '../../layout/__tests__/vnodeWalk';
import { latestTauriAppVersion, releaseCheck, releaseNoticeView } from '../../../store/store';
import {
  releaseNoticeBadge,
  systemTabBadge,
  updateBadge,
  systemAttentionBadge,
} from '../../../store/systemAttentionBadge';
import { drawerClosing, drawerOpen } from '../../layout/Drawer';

const DRAWER = readFileSync(
  fileURLToPath(new URL('../../layout/Drawer.tsx', import.meta.url)), 'utf8',
);
const SETTINGS = readFileSync(
  fileURLToPath(new URL('../../settings/SettingsView.tsx', import.meta.url)), 'utf8',
);
const SYSTEM = readFileSync(
  fileURLToPath(new URL('../../settings/SystemPage.tsx', import.meta.url)), 'utf8',
);
const BADGE = readFileSync(
  fileURLToPath(new URL('../SystemAttentionBadge.tsx', import.meta.url)), 'utf8',
);

/** Owe one notice, the shortest way to raise the badge. */
function oweOne(): void {
  releaseNoticeView.value = {
    status: 'loaded',
    data: {
      notices: [{ id: 'a', since: '2.0.0', title: 'Audit', body: 'Run it.', resolved: false }],
      next_id: 'a',
    },
  };
}

/** The component takes the answer rather than reading it, so the helper reads
 *  the union for it. That is what the upstream hosts pass. */
function badge(placement: 'corner' | 'inline'): VNode | null {
  return SystemAttentionBadge({ placement, label: systemAttentionBadge() }) as VNode | null;
}

beforeEach(() => {
  releaseCheck.value = null;
  latestTauriAppVersion.value = null;
  releaseNoticeView.value = { status: 'not-loaded' };
  drawerOpen.value = false;
  drawerClosing.value = false;
});

describe('the mark itself', () => {
  it('draws nothing at all on a quiet workspace', () => {
    // Not an empty box: nothing holds its space, so no row moves when news
    // arrives or clears.
    expect(badge('corner')).toBe(null);
    expect(badge('inline')).toBe(null);
  });

  it('is one dot, hidden from assistive tech, on either placement', () => {
    oweOne();
    const corner = badge('corner') as AnyVNode;
    expect(corner.props['aria-hidden']).toBe('true');
    const slot = badge('inline') as AnyVNode;
    expect(slot.props['aria-hidden']).toBe('true');
    expect(findByClass(slot, 'system-attention-badge')).toHaveLength(1);
  });

  it('adds no text to its host, on either placement', () => {
    oweOne();
    // Off-screen text inside the badge would fuse with the label it marks, in
    // `textContent` and in the accessible name ("Settings1 thing to do"). The
    // hosts say the words instead.
    expect(textOf(badge('inline'))).toBe('');
    expect(textOf(badge('corner'))).toBe('');
  });

  it('rides `.badge` at a corner, so the header bar repaints it like the rest', () => {
    oweOne();
    // `.app-header .badge` repaints every badge for the bar and rings it, and
    // the ring is what stops a mark fusing with the glyph it sits on.
    const klass = String((badge('corner') as AnyVNode).props.class);
    expect(klass.split(' ')).toContain('badge');
  });
});

describe('the menu hamburger', () => {
  it('names itself as before while there is no news', () => {
    const button = HamburgerButton() as AnyVNode;
    expect(button.props['aria-label']).toBe('Open menu');
    expect(button.props['data-tooltip']).toBe('Open menu');
  });

  it('speaks the news in its own label, and its tooltip', () => {
    oweOne();
    const button = HamburgerButton() as AnyVNode;
    expect(button.props['aria-label']).toBe('Open menu · 1 thing to do');
    expect(button.props['data-tooltip']).toBe('Open menu · 1 thing to do');
  });

  it('keeps the news beside the close action while the drawer is open', () => {
    oweOne();
    drawerOpen.value = true;
    expect((HamburgerButton() as AnyVNode).props['aria-label']).toBe('Close menu · 1 thing to do');
  });

  it('hosts the mark, whatever the mark then decides to draw', () => {
    const kids = (HamburgerButton() as AnyVNode).props.children as unknown[];
    expect(kids.some((k) => (k as AnyVNode | null)?.type === SystemAttentionBadge)).toBe(true);
  });
});

describe('the rows that carry the mark inline', () => {
  it('puts it on the menu drawer\'s Settings row, as the union', () => {
    const row = DRAWER.indexOf('\n          Settings\n');
    expect(row, 'the Settings row').toBeGreaterThan(-1);
    expect(DRAWER.slice(row, row + 160))
      .toContain('<SystemAttentionBadge placement="inline" label={systemAttentionBadge()} />');
  });

  it('puts it on the System row alone, never on every Settings category', () => {
    expect(SETTINGS)
      .toContain('{key === \'system\' && <SystemAttentionBadge placement="inline" label={news} />}');
  });

  // The mark is decorative, so a host that CAN name itself has to say the
  // words. The drawer row cannot: it is a role-less div, and the hamburger
  // that opened it has already said them.
  it('makes the badged buttons speak the sentence', () => {
    expect(SETTINGS).toContain('aria-label={key === \'system\' && news ?');
    expect(SYSTEM).toContain('aria-label={badge ? `${item.label} · ${badge}` : undefined}');
  });

  it('adds no off-screen text to any row', () => {
    // `.drawer-item.active` is asserted to read exactly "Settings" by
    // `e2e/settings-backup-navigation-desktop.spec.ts`.
    expect(BADGE).not.toContain('visually-hidden');
  });
});

/**
 * The last step of the path splits, because the two causes sit on two tabs.
 *
 * A mark on a tab promises work on THAT tab. An update must never dot Release
 * Notices, and an owed notice must never dot What's New.
 */
describe('the two System tabs that can owe something', () => {
  it('dots Release Notices for an owed notice, and nothing else', () => {
    oweOne();
    expect(systemTabBadge('release-notices')).toBe('1 thing to do');
    expect(systemTabBadge('whats-new')).toBe(null);
    expect(systemTabBadge('backup')).toBe(null);
    // The path above the two tabs still leads to both, so it keeps the union.
    expect(systemAttentionBadge()).toBe('1 thing to do');
  });

  it('dots What\'s New for an available update, and nothing else', () => {
    releaseCheck.value = { latest: { version: '9.9.9' } } as typeof releaseCheck.value;
    expect(systemTabBadge('whats-new')).toBe('Lucidos 9.9.9 available');
    expect(systemTabBadge('whats-new')).toBe(updateBadge());
    expect(systemTabBadge('release-notices')).toBe(releaseNoticeBadge());
    expect(systemTabBadge('release-notices')).toBe(null);
  });

  it('leaves the switcher unable to mark a tab it has no sentence for', () => {
    // Every other subpanel answers null, so a new one is unmarked until
    // somebody names a source for it.
    expect(systemTabBadge('thread-queue')).toBe(null);
    expect(systemTabBadge('overview')).toBe(null);
  });
});
