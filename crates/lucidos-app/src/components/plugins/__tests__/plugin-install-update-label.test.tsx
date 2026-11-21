// @vitest-environment jsdom
/**
 * Confirming over an installed plugin updates it, so the panel says Update.
 * A fresh install says Install, even when it would replace a file on disk.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginInstallPanel } from '../PluginInstallPanel';
import { panelOverlay } from '../../../store/store';
import type { PluginInstallRequest } from '../../../store/types';

const base: PluginInstallRequest = {
  install_id: 'i-1',
  source: 'https://example.com/trigger-timeline',
  source_type: 'git',
  manifest: { description: 'Shows trigger runs.' },
  files: ['apps/trigger-timeline/index.html'],
  overwrites: [],
  plugin_id: 'trigger-timeline',
  plugin_version: '0.2.0',
  plugin_name: 'Trigger Timeline',
};

let host: HTMLDivElement | null = null;

function show(request: PluginInstallRequest): HTMLDivElement {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  host = document.createElement('div');
  render(<PluginInstallPanel />, host);
  return host;
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
  panelOverlay.value = null;
});

describe('the install panel button', () => {
  it('says Update when the plugin is installed', () => {
    const el = show({ ...base, is_update: true });
    expect(el.querySelector('.action-btn-confirm')?.textContent).toBe('Update');
    expect(el.textContent).toContain('Update plugin');
  });

  it('says Install for a fresh install that collides with a file', () => {
    const el = show({ ...base, is_update: false, overwrites: ['apps/trigger-timeline/index.html'] });
    expect(el.querySelector('.action-btn-confirm')?.textContent).toBe('Install');
    expect(el.textContent).toContain('Install plugin');
  });
});
