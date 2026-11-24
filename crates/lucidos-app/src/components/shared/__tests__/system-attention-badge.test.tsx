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
  systemPageBadge,
  updateBadge,
  systemAttentionBadge,
} from '../../../store/systemAttentionBadge';
import { drawerClosing, drawerOpen } from '../../layout/drawerState';
import { tooltipWithShortcut } from '../../../store/actions/keybindings';

const DRAWER = readFileSync(
  fileURLToPath(new URL('../../layout/Drawer.tsx', import.meta.url)), 'utf8',
);
const SETTINGS_HOME = readFileSync(
  fileURLToPath(new URL('../../settings/SettingsHome.tsx', import.meta.url)), 'utf8',
);
const SUBMENU = readFileSync(
  fileURLToPath(new URL('../../settings/SystemSubmenu.tsx', import.meta.url)), 'utf8',
);
const NAV_ROW = readFileSync(
  fileURLToPath(new URL('../../settings/SettingsNavRow.tsx', import.meta.url)), 'utf8',
);
const BADGE = readFileSync(
  fileURLToPath(new URL('../SystemAttentionBadge.tsx', import.meta.url)), 'utf8',
);

function oweOne(): void {
  releaseNoticeView.value = {
    status: 'loaded',
    data: {
      releases: [{
        since: '2.0.0',
        items: [{ id: 'a', since: '2.0.0', title: 'Audit', summary: 'Review it.', body: 'Run it.', resolved: false }],
        resolved: false,
        action_prompt: null,
      }],
      owed_through: 'a',
      action_prompt: null,
    },
  };
}

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
    expect(textOf(badge('inline'))).toBe('');
    expect(textOf(badge('corner'))).toBe('');
  });

  it('rides `.badge` at a corner, so the header bar repaints it like the rest', () => {
    oweOne();
    const klass = String((badge('corner') as AnyVNode).props.class);
    expect(klass.split(' ')).toContain('badge');
  });
});

describe('the menu hamburger', () => {
  it('names itself as before while there is no news', () => {
    const button = HamburgerButton() as AnyVNode;
    expect(button.props['aria-label']).toBe('Open menu');
    expect(button.props['data-tooltip']).toBe(tooltipWithShortcut('Open menu', 'toggleMenuDrawer'));
  });

  it('speaks the news in its own label, and its tooltip', () => {
    oweOne();
    const button = HamburgerButton() as AnyVNode;
    expect(button.props['aria-label']).toBe('Open menu · 1 thing to do');
    expect(button.props['data-tooltip']).toBe(tooltipWithShortcut('Open menu · 1 thing to do', 'toggleMenuDrawer'));
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
    const row = DRAWER.indexOf('\n          {MENU_ITEM_LABELS.settings}\n');
    expect(row, 'the Settings row').toBeGreaterThan(-1);
    expect(DRAWER.slice(row, row + 160))
      .toContain('<SystemAttentionBadge placement="inline" label={systemAttentionBadge()} />');
  });

  it('puts it on the System row alone, never on every Settings category', () => {
    expect(SETTINGS_HOME).toContain('badge={key === \'system\' ? news : null}');
  });

  it('marks a System submenu row by its own source, never the union', () => {
    expect(SUBMENU).toContain('badge={systemPageBadge(key)}');
    expect(SUBMENU).not.toContain('systemAttentionBadge(');
  });

  it('makes the badged row speak the sentence, and hug the word with the mark', () => {
    expect(NAV_ROW).toContain('aria-label={badge ? `${label} · ${badge}` : undefined}');
    expect(NAV_ROW)
      .toContain('<span>{label}<SystemAttentionBadge placement="inline" label={badge} /></span>');
  });

  it('adds no off-screen text to any row', () => {
    expect(BADGE).not.toContain('visually-hidden');
  });
});

describe('the two System sub-pages that can owe something', () => {
  it('dots Release Notices for an owed notice, and nothing else', () => {
    oweOne();
    expect(systemPageBadge('release-notices')).toBe('1 thing to do');
    expect(systemPageBadge('whats-new')).toBe(null);
    expect(systemPageBadge('backup')).toBe(null);
    expect(systemAttentionBadge()).toBe('1 thing to do');
  });

  it('dots What\'s New for an available update, and nothing else', () => {
    releaseCheck.value = { latest: { version: '9.9.9' } } as typeof releaseCheck.value;
    expect(systemPageBadge('whats-new')).toBe('Lucidos 9.9.9 available');
    expect(systemPageBadge('whats-new')).toBe(updateBadge());
    expect(systemPageBadge('release-notices')).toBe(releaseNoticeBadge());
    expect(systemPageBadge('release-notices')).toBe(null);
  });

  it('leaves the submenu unable to mark a row it has no sentence for', () => {
    expect(systemPageBadge('thread-queue')).toBe(null);
    expect(systemPageBadge('system-overview')).toBe(null);
  });
});
