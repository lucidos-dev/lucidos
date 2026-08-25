/**
 * What the install panel says about local changes an update is about to touch.
 *
 * Two decisions are pulled out as pure functions. The confirm panel holds its
 * keep control in a hook, and the suite's VNode walk stops at function
 * components. The receipt half has no hooks, so it is rendered directly, the
 * same way `plugin-receipts.test.tsx` does.
 */
import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  PluginInstallReceiptPanel,
  localChangeLabel,
  plainOverwrites,
} from '../PluginInstallPanel';
import type { PluginInstallForm } from '../../../store/store';

function text(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(text).join('');
  const v = node as VNode<{ children?: ComponentChildren }>;
  // Stop at function components, exactly as the receipts suite does: descending
  // would invoke render-time hooks outside a real render.
  if (typeof v.type === 'function') return '';
  return text(v.props?.children);
}

const request = {
  install_id: 'i-1',
  source: 'git://example.com/email-triage',
  source_type: 'git' as const,
  manifest: { description: 'Triages email.' },
  files: ['knowhow/email-triage.md', 'knowhow/new.md'],
  overwrites: ['knowhow/email-triage.md'],
  plugin_id: 'email-triage',
  plugin_version: '2.0.0',
  plugin_name: 'Email Triage',
};

function receipt(local?: PluginInstallForm['installed']): PluginInstallForm {
  return { type: 'plugin-install', request, installed: local };
}

describe('the keep control rewrites what every row promises', () => {
  it('states the real per-file outcome while it is on', () => {
    expect(localChangeLabel('merged', true)).toContain('Kept');
    expect(localChangeLabel('conflict', true)).toContain('Cannot merge');
    expect(localChangeLabel('replaced', true)).toContain('Replaced');
  });

  it('reads every row as replaced once it is cleared', () => {
    // Clearing it sends keep_local_changes=false, which collapses every outcome
    // to a replace. A row still claiming "Kept" would promise the opposite of
    // the request the button is about to send.
    for (const outcome of ['merged', 'conflict', 'replaced'] as const) {
      expect(localChangeLabel(outcome, false)).toBe('Replaced, your version saved aside');
    }
  });

  it('leaves a restore alone either way', () => {
    // The user deleted the file, so there is no edit to keep or drop, and
    // nothing is saved aside. Calling it "saved aside" would be a false
    // promise, which is the whole reason it is its own outcome.
    for (const keep of [true, false]) {
      expect(localChangeLabel('restored', keep)).toContain('brings it back');
      expect(localChangeLabel('restored', keep)).not.toContain('saved aside');
    }
  });
});

describe('the blunt overwrite list', () => {
  it('drops paths that have their own outcome row', () => {
    expect(
      plainOverwrites(
        ['knowhow/email-triage.md', 'knowhow/other.md'],
        [{ path: 'knowhow/email-triage.md' }],
      ),
    ).toEqual(['knowhow/other.md']);
  });

  it('keeps everything when nothing was edited', () => {
    expect(plainOverwrites(['a.md', 'b.md'], [])).toEqual(['a.md', 'b.md']);
  });
});

describe('the receipt', () => {
  it('says what happened to each edit and where the lost ones went', () => {
    const body = text(
      PluginInstallReceiptPanel({
        form: receipt({
          at: '2026-08-19T10:00:00.000Z',
          summary: 'Installed Email Triage v2.0.0',
          installed_files: ['knowhow/email-triage.md'],
          local_changes: {
            merged: ['knowhow/email-triage.md'],
            conflicted: ['triggers/email-triage/intents/triage.md'],
            replaced: [],
            restored: ['knowhow/email-triage-extra.md'],
            saved_paths: [
              'artifacts/plugin-local-changes/email-triage/v2.0.0/triggers/email-triage/intents/triage.md',
            ],
          },
        }),
      }),
    );
    expect(body).toContain('Merged into the new version: knowhow/email-triage.md');
    expect(body).toContain('Could not merge: triggers/email-triage/intents/triage.md');
    expect(body).toContain('saved under');
    expect(body).toContain('brings them back');
  });

  it('says nothing about local changes when the install met none', () => {
    const body = text(
      PluginInstallReceiptPanel({
        form: receipt({
          at: '2026-08-19T10:00:00.000Z',
          summary: 'Installed Email Triage v2.0.0',
          installed_files: ['knowhow/email-triage.md'],
        }),
      }),
    );
    expect(body).not.toContain('Your local changes');
  });
});
