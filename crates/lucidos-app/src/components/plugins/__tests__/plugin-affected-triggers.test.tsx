// @vitest-environment jsdom
/**
 * A plugin update or uninstall that removes a file a user's trigger relies on
 * warns about it in the panel and on the receipt. The engine leaves the
 * trigger as it is, so this list is the user's only sight of it before confirm.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginInstallPanel } from '../PluginInstallPanel';
import { PluginUninstallPanel } from '../PluginUninstallPanel';
import { affectedReasonLabel } from '../AffectedTriggerList';
import { panelOverlay } from '../../../store/store';
import type { InlineForm } from '../../../store/store';
import type {
  AffectedTrigger,
  PluginInstallRequest,
  PluginUninstallRequest,
} from '../../../store/types';

const removedKnowhow = 'apps/token-cost/knowhow/daily-price-check.md';

const affected: AffectedTrigger[] = [{
  trigger_id: 't-1',
  trigger_name: 'Token Cost daily price check',
  paths: [{ path: removedKnowhow, reason: 'owning-app' }],
}];

const update: PluginInstallRequest = {
  install_id: 'i-1',
  source: 'https://example.com/token-cost',
  source_type: 'git',
  manifest: { description: 'Tracks token cost.' },
  files: ['apps/token-cost/index.html'],
  overwrites: ['apps/token-cost/index.html'],
  removes: [removedKnowhow],
  is_update: true,
  plugin_id: 'token-cost',
  plugin_version: '0.6.0',
  plugin_name: 'Token Cost',
};

const uninstall: PluginUninstallRequest = {
  uninstall_id: 'u-1',
  plugin_id: 'token-cost',
  plugin_version: '0.6.0',
  plugin_name: 'Token Cost',
  files_present: [removedKnowhow],
  files_missing: [],
};

let host: HTMLDivElement | null = null;

function show(form: InlineForm): HTMLDivElement {
  panelOverlay.value = { type: 'form', form };
  host = document.createElement('div');
  render(form.type === 'plugin-install' ? <PluginInstallPanel /> : <PluginUninstallPanel />, host);
  return host;
}

function section(el: HTMLElement, label: string): HTMLElement | null {
  const toggle = [...el.querySelectorAll('.plugin-install-section-toggle')]
    .find((b) => b.querySelector('.plugin-install-label')?.textContent === label);
  return toggle?.closest('section') ?? null;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
  panelOverlay.value = null;
});

describe('affectedReasonLabel', () => {
  it('names the app from the path for an owning-app match', () => {
    expect(affectedReasonLabel({ path: removedKnowhow, reason: 'owning-app' }))
      .toBe('Belongs to the token-cost app');
  });

  it('labels the other two reasons', () => {
    expect(affectedReasonLabel({ path: 'knowhow/x.md', reason: 'named-in-run' }))
      .toBe('Named in what it runs');
    expect(affectedReasonLabel({ path: 'triggers/x/knowhow/y.md', reason: 'trigger-knowhow' }))
      .toBe('Its own know-how');
  });
});

describe('the update panel', () => {
  it('lists each affected trigger, the removed path and why, open', () => {
    const el = show({ type: 'plugin-install', request: { ...update, affected_triggers: affected } });
    const s = section(el, 'Triggers that use these files');
    expect(s).not.toBeNull();
    expect(s!.querySelector('[aria-expanded]')?.getAttribute('aria-expanded')).toBe('true');
    expect(s!.textContent).toContain('Token Cost daily price check');
    expect(s!.textContent).toContain(removedKnowhow);
    expect(s!.textContent).toContain('Belongs to the token-cost app');
    expect(s!.textContent).toContain('leaves them as they are');
  });

  it('shows no section when nothing is affected', () => {
    const el = show({ type: 'plugin-install', request: { ...update, affected_triggers: [] } });
    expect(section(el, 'Triggers that use these files')).toBeNull();
  });
});

describe('the uninstall panel', () => {
  it('lists the affected triggers', () => {
    const el = show({ type: 'plugin-uninstall', request: { ...uninstall, affected_triggers: affected } });
    const s = section(el, 'Triggers that use these files');
    expect(s?.textContent).toContain('Token Cost daily price check');
    expect(s?.textContent).toContain('uninstalling leaves them as they are');
  });
});

describe('the receipts', () => {
  it('list the triggers to check after an update', () => {
    const el = show({
      type: 'plugin-install',
      request: update,
      installed: {
        at: '2026-10-21T10:00:00.000Z',
        summary: 'Updated Token Cost',
        installed_files: [],
        removed_files: [removedKnowhow],
        affected_triggers: affected,
      },
    });
    expect(section(el, 'Triggers to check')?.textContent).toContain('Token Cost daily price check');
  });

  it('list the triggers to check after an uninstall', () => {
    const el = show({
      type: 'plugin-uninstall',
      request: uninstall,
      removed: {
        at: '2026-10-21T10:00:00.000Z',
        summary: 'Removed Token Cost',
        files_deleted: [removedKnowhow],
        files_missing: [],
        affected_triggers: affected,
      },
    });
    expect(section(el, 'Triggers to check')?.textContent).toContain('Token Cost daily price check');
  });

  it('show no section when nothing is affected', () => {
    const el = show({
      type: 'plugin-uninstall',
      request: uninstall,
      removed: {
        at: '2026-10-21T10:00:00.000Z',
        summary: 'Removed Token Cost',
        files_deleted: [removedKnowhow],
        files_missing: [],
      },
    });
    expect(section(el, 'Triggers to check')).toBeNull();
  });
});
