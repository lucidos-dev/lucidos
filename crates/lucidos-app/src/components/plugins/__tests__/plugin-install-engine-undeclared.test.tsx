// @vitest-environment jsdom
/**
 * The confirmation panel's quiet note for a plugin whose manifest declares no
 * `engine` requirement. Visible only: the Install button stays live.
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

describe('the install panel and a missing engine requirement', () => {
  it('says so in one muted note, and leaves Install live', () => {
    const el = show({ ...base, engine_requirement: null });
    const note = el.querySelector('[data-role="engine-undeclared"]');
    expect(note?.textContent).toBe("This plugin doesn't say which Lucidos version it needs.");
    expect(note?.className).toBe('plugin-install-note');
    const install = el.querySelector<HTMLButtonElement>('.action-btn-confirm');
    expect(install?.textContent).toBe('Install');
    expect(install?.disabled).toBe(false);
  });

  it('shows no note when the manifest declares one', () => {
    for (const engine_requirement of ['>=0.30.0', '']) {
      const el = show({ ...base, engine_requirement });
      expect(el.querySelector('[data-role="engine-undeclared"]')).toBeNull();
      render(null, el);
    }
  });
});
