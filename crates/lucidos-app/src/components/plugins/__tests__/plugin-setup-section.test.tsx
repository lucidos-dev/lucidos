// @vitest-environment jsdom
/**
 * A plugin's setup steps are instructions for the Lucidos Agent, which runs
 * them in a setup thread. Both panels must say so in the heading and keep the
 * steps folded, or they read as a to-do list for the user.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginInstallPanel, PluginInstallReceiptPanel } from '../PluginInstallPanel';
import { panelOverlay } from '../../../store/store';
import type { PluginInstallForm } from '../../../store/store';
import type { PluginInstallRequest } from '../../../store/types';

const request: PluginInstallRequest = {
  install_id: 'i-1',
  source: 'https://example.com/habit-tracker',
  source_type: 'git',
  manifest: { description: 'Tracks habits.' },
  files: ['apps/habit-tracker/manifest.json'],
  overwrites: [],
  setup: 'Create the daily reminder trigger.',
  plugin_id: 'habit-tracker',
  plugin_version: '0.1.0',
  plugin_name: 'Habit Tracker',
};

let host: HTMLDivElement | null = null;

function mount(node: preact.ComponentChild): HTMLDivElement {
  host = document.createElement('div');
  render(node, host);
  return host;
}

function setupToggle(root: HTMLElement): HTMLButtonElement {
  const toggle = [...root.querySelectorAll<HTMLButtonElement>('.plugin-install-section-toggle')]
    .find((b) => b.textContent?.includes('setup steps'));
  if (!toggle) throw new Error('no setup steps heading');
  return toggle;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
  panelOverlay.value = null;
});

describe('the setup steps section', () => {
  it('says the agent runs them, folded, on the confirm panel', () => {
    panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
    const toggle = setupToggle(mount(<PluginInstallPanel />));
    expect(toggle.textContent).toBe('Agent setup steps · run for you');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });

  it('says the agent runs them, folded, on the receipt', () => {
    const form: PluginInstallForm = {
      type: 'plugin-install',
      request,
      installed: { at: '2026-01-01T00:00:00Z', summary: 'Installed.', installed_files: [] },
    };
    const toggle = setupToggle(mount(<PluginInstallReceiptPanel form={form} />));
    expect(toggle.textContent).toBe('Agent setup steps · run for you');
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
  });

  it('promises no run on an update whose setup seeds no thread', () => {
    const update = { ...request, is_update: true, setup_run: null };
    panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request: update } };
    expect(setupToggle(mount(<PluginInstallPanel />)).textContent).toBe('Agent setup steps');
    render(null, host!);
    const form: PluginInstallForm = {
      type: 'plugin-install',
      request: update,
      installed: { at: '2026-01-01T00:00:00Z', summary: 'Updated.', installed_files: [] },
    };
    expect(setupToggle(mount(<PluginInstallReceiptPanel form={form} />)).textContent).toBe('Agent setup steps');
  });
});
