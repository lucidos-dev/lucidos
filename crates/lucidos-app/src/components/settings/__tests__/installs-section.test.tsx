/**
 * Settings, System, Overview: what the Installs list says, and when it warns.
 *
 * The two rules are the ones a user feels. Every install is listed, always, so
 * "why am I on an old engine" is answerable from this one screen. And only a
 * contended port earns a warning. A developer running a source checkout beside
 * the app is never nagged about a setup that works.
 *
 * Pure builders rather than a mounted render, the shape
 * `backup-health-card.test.tsx` uses: the page pulls in the whole store.
 */
import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { installRow, installConflictNotice } from '../InstallsSection';
import type { InstallRecord } from '../../../api/client/control';

/** Flatten a vnode tree to a string, keeping `class` so a per-state rule can be
 *  asserted. Mirrors the helper in `backup-health-card.test.tsx`. Component
 *  vnodes keep their tag, since the row draws most of its text through
 *  `<SkText>` and invoking that here would throw on its hook. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown>>;
  const props = (v.props ?? {}) as Record<string, unknown>;
  const cls = typeof props.class === 'string' ? ` class="${props.class}"` : '';
  const tag = typeof v.type === 'string' ? v.type : ((v.type as { name?: string })?.name ?? 'C');
  return `<${tag}${cls}>${vnodeToText(props.children as ComponentChildren)}</${tag}>`;
}

function record(over: Partial<InstallRecord> = {}): InstallRecord {
  return {
    kind: 'desktop-app',
    name: 'Lucidos.app in /Applications',
    root: '/Applications/Lucidos.app',
    version: '0.36.0',
    data_dir: '/Users/me/Library/Application Support/com.lucidos.app',
    port: 5252,
    agents: [],
    running_here: false,
    removal: 'Open Lucidos and choose Uninstall Lucidos from the Lucidos menu.',
    ...over,
  };
}

describe('installRow', () => {
  it('names the install, its kind, its version and its port', () => {
    const text = vnodeToText(installRow(record()));

    expect(text).toContain('Lucidos.app in /Applications');
    expect(text).toContain('Desktop app');
    expect(text).toContain('version 0.36.0');
    expect(text).toContain('port 5252');
    expect(text).toContain('/Applications/Lucidos.app');
  });

  // The whole point of the screen is that every number on it can be trusted.
  // A blank would read as zero, and a plausible default would be a lie.
  it('says "unknown" rather than guessing a version or a port', () => {
    const text = vnodeToText(installRow(record({ version: null, port: null })));

    expect(text).toContain('version unknown');
    expect(text).toContain('port unknown');
  });

  it('marks the one serving this workspace', () => {
    expect(vnodeToText(installRow(record({ running_here: true })))).toContain(
      'serving this workspace',
    );
    expect(vnodeToText(installRow(record()))).not.toContain('serving this workspace');
  });

  // A registered job is how a forgotten install keeps coming back after a
  // reboot, so the row names every one of them.
  it('names each job that can start the install on its own', () => {
    const text = vnodeToText(
      installRow(
        record({
          kind: 'headless-installer',
          name: 'install.sh instance "default"',
          agents: [
            {
              label: 'com.lucidos.gateway.default',
              path: '/Users/me/Library/LaunchAgents/com.lucidos.gateway.default.plist',
              manager: 'launchd',
            },
          ],
        }),
      ),
    );

    expect(text).toContain('Installer');
    expect(text).toContain('Starts on its own');
    expect(text).toContain('com.lucidos.gateway.default');
  });

  // The placeholder is the same markup, so it cannot drift from the row. It
  // must not render an install's fields, having none.
  it('draws a placeholder from its own markup', () => {
    const text = vnodeToText(installRow(undefined, true));

    expect(text).toContain('list-row');
    expect(text).toContain('version unknown');
    expect(text).not.toContain('Starts on its own');
  });
});

describe('installConflictNotice', () => {
  it('names the port and how many installs want it', () => {
    const text = vnodeToText(
      installConflictNotice({
        port: 5252,
        installs: ['Lucidos.app in /Applications', 'install.sh instance "default"'],
      }),
    );

    expect(text).toContain('port 5252');
    expect(text).toContain('2 installs');
    expect(text).toContain('whichever started first wins');
  });
});
