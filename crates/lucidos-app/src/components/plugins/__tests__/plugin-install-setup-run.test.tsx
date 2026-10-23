// @vitest-environment jsdom
/**
 * The confirm panel says whether an update runs setup again. The line sits
 * under the Agent setup steps heading, so it reads while the section is folded.
 */
import { afterEach, describe, it, expect } from 'vitest';
import { render } from 'preact';
import { PluginInstallPanel } from '../PluginInstallPanel';
import { panelOverlay } from '../../../store/store';
import type { PluginInstallRequest } from '../../../store/types';

const base: PluginInstallRequest = {
  install_id: 'i-1',
  source: 'https://example.com/skill-pack',
  source_type: 'git',
  manifest: { description: 'Ships a skill.' },
  files: ['knowhow/skill-pack/skill.md'],
  overwrites: [],
  setup: 'Copy the skill to the skills folder.',
  is_update: true,
  plugin_id: 'skill-pack',
  plugin_version: '0.2.0',
  plugin_name: 'Skill Pack',
};

let host: HTMLDivElement | null = null;

function noteOf(request: PluginInstallRequest): string | undefined {
  panelOverlay.value = { type: 'form', form: { type: 'plugin-install', request } };
  host = document.createElement('div');
  render(<PluginInstallPanel />, host);
  const notes = [...host.querySelectorAll('.plugin-install-note')].map((n) => n.textContent ?? '');
  return notes.find((t) => t.startsWith('Setup '));
}

afterEach(() => {
  if (host) render(null, host);
  host = null;
  panelOverlay.value = null;
});

describe('the install panel and setup on an update', () => {
  it('says setup runs again because the steps changed', () => {
    const run = { kind: 'update', from_version: '0.1.0', cause: { kind: 'instructions_changed' } } as const;
    expect(noteOf({ ...base, setup_run: run })).toBe('Setup runs again after this update: the steps changed.');
  });

  it('says the author asks for it, and quotes the reason', () => {
    const run = {
      kind: 'update',
      from_version: '0.1.0',
      cause: { kind: 'revision_raised', revision: 2, reason: 'The skill changed.' },
    } as const;
    expect(noteOf({ ...base, setup_run: run })).toBe(
      "Setup runs again after this update: the author asks for it. Author's note: The skill changed.",
    );
  });

  it('names both causes', () => {
    const run = { kind: 'update', cause: { kind: 'both', revision: 3 } } as const;
    expect(noteOf({ ...base, setup_run: run })).toBe(
      'Setup runs again after this update: the steps changed and the author asks for it.',
    );
  });

  it('says setup does not run again when the engine would seed no thread', () => {
    expect(noteOf({ ...base, setup_run: null })).toBe('Setup does not run again: nothing in it changed.');
  });

  it('says nothing on a fresh install or a request staged before the field existed', () => {
    expect(noteOf({ ...base, is_update: false, setup_run: { kind: 'fresh_install' } })).toBeUndefined();
    render(null, host!);
    expect(noteOf({ ...base })).toBeUndefined();
  });
});
