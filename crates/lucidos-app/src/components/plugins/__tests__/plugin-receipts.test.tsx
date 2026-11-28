import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { PluginInstallPanel, PluginInstallReceiptPanel, installedApps } from '../PluginInstallPanel';
import type { App } from '../../../store/types';
import { PluginUninstallPanel, PluginUninstallReceiptPanel } from '../PluginUninstallPanel';
import { PluginFileList } from '../PluginFileList';
import { appsList, panelOverlay } from '../../../store/store';
import type { InlineForm, PluginInstallForm, PluginUninstallForm } from '../../../store/store';

type AnyVNode = VNode<Record<string, unknown>>;

interface Walked {
  text: string[];
  buttons: AnyVNode[];
  fileLists: { label: string; files: string[] }[];
  components: Set<unknown>;
}

function emptyAcc(): Walked {
  return { text: [], buttons: [], fileLists: [], components: new Set() };
}

function walk(node: ComponentChildren, acc: Walked): void {
  if (node === null || node === undefined || typeof node === 'boolean') return;
  if (typeof node === 'string' || typeof node === 'number') {
    acc.text.push(String(node));
    return;
  }
  if (Array.isArray(node)) {
    node.forEach((n) => walk(n, acc));
    return;
  }
  const v = node as AnyVNode;
  if (typeof v.type === 'function') {
    acc.components.add(v.type);
    if (v.type === PluginFileList) {
      acc.fileLists.push({
        label: v.props.label as string,
        files: v.props.files as string[],
      });
    }
    return;
  }
  if (v.type === 'button') acc.buttons.push(v);
  walk(v.props?.children as ComponentChildren, acc);
}

function render(node: ComponentChildren): Walked {
  const acc = emptyAcc();
  walk(node, acc);
  return acc;
}

function buttonLabels(acc: Walked): string[] {
  return acc.buttons.map((b) => {
    const inner = emptyAcc();
    walk(b.props.children as ComponentChildren, inner);
    return inner.text.join('').trim();
  });
}

const installRequest = {
  install_id: 'i-1',
  source: 'git://example.com/habit-tracker',
  source_type: 'git' as const,
  manifest: { description: 'Tracks habits.' },
  files: ['apps/habit-tracker/manifest.json', 'apps/habit-tracker/index.html'],
  overwrites: [],
  plugin_id: 'habit-tracker',
  plugin_version: '1.0.0',
  plugin_name: 'Habit Tracker',
};

const uninstallRequest = {
  uninstall_id: 'u-1',
  plugin_id: 'habit-tracker',
  plugin_version: '1.0.0',
  plugin_name: 'Habit Tracker',
  files_present: ['apps/habit-tracker/manifest.json', 'apps/habit-tracker/index.html'],
  files_missing: [],
};

function uninstallForm(removed?: PluginUninstallForm['removed']): PluginUninstallForm {
  return { type: 'plugin-uninstall', request: uninstallRequest, removed };
}

function installForm(installed?: PluginInstallForm['installed']): PluginInstallForm {
  return { type: 'plugin-install', request: installRequest, installed };
}

function open(form: InlineForm): void {
  panelOverlay.value = { type: 'form', form };
}

beforeEach(() => {
  panelOverlay.value = null;
  appsList.value = { status: 'not-loaded' };
});

describe('plugin panel dispatchers', () => {
  it('route to the receipt once the marker is on the form, and to the confirm before', () => {
    open(uninstallForm());
    expect(render(PluginUninstallPanel()).components).not.toContain(PluginUninstallReceiptPanel);

    open(uninstallForm({
      at: '2026-08-06T10:00:00.000Z',
      summary: 'Removed Habit Tracker',
      files_deleted: [],
      files_missing: [],
    }));
    expect(render(PluginUninstallPanel()).components).toContain(PluginUninstallReceiptPanel);

    open(installForm());
    expect(render(PluginInstallPanel()).components).not.toContain(PluginInstallReceiptPanel);

    open(installForm({
      at: '2026-08-06T10:00:00.000Z',
      summary: 'Installed Habit Tracker',
      installed_files: [],
    }));
    expect(render(PluginInstallPanel()).components).toContain(PluginInstallReceiptPanel);
  });
});

describe('plugin uninstall receipt', () => {
  it('reports what the ENGINE deleted, not what the staged request listed', () => {
    const acc = render(PluginUninstallReceiptPanel({
      form: uninstallForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Removed Habit Tracker',
        files_deleted: ['apps/habit-tracker/manifest.json'],
        files_missing: ['apps/habit-tracker/index.html'],
      }),
    }));

    expect(acc.fileLists).toEqual([
      { label: 'Deleted', files: ['apps/habit-tracker/manifest.json'] },
      { label: 'Already gone', files: ['apps/habit-tracker/index.html'] },
    ]);
    expect(acc.text.join(' ')).toContain('Removed Habit Tracker');
    expect(acc.text).toContain('Uninstalled');
  });

  it('offers no buttons: the files are gone and the staged id is popped', () => {
    const acc = render(PluginUninstallReceiptPanel({
      form: uninstallForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Removed Habit Tracker',
        files_deleted: [],
        files_missing: [],
      }),
    }));

    expect(buttonLabels(acc)).toEqual([]);
  });
});

describe('plugin install receipt', () => {
  it('reports the files the ENGINE wrote, not the staged file list', () => {
    const acc = render(PluginInstallReceiptPanel({
      form: installForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Installed Habit Tracker',
        installed_files: ['apps/habit-tracker/manifest.json'],
      }),
    }));

    expect(acc.fileLists).toEqual([
      { label: 'Files installed', files: ['apps/habit-tracker/manifest.json'] },
    ]);
    expect(acc.text.join(' ')).toContain('Installed Habit Tracker');
    expect(acc.text).toContain('Installed');
  });

  it('offers no buttons when the plugin installed no app', () => {
    const acc = render(PluginInstallReceiptPanel({
      form: installForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Installed Habit Tracker',
        installed_files: ['knowhow/habits/tracking.md'],
      }),
    }));

    expect(buttonLabels(acc)).toEqual([]);
  });

  it('offers Open app when the plugin installed one app', () => {
    const acc = render(PluginInstallReceiptPanel({
      form: installForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Installed Habit Tracker',
        installed_files: ['apps/habit-tracker/manifest.json', 'apps/habit-tracker/index.html'],
      }),
    }));

    expect(buttonLabels(acc)).toEqual(['Open app']);
  });
});

describe('installedApps', () => {
  const files = [
    'apps/habit-tracker/index.html',
    'apps/habit-tracker/manifest.json',
    'apps/streak-widget/manifest.json',
    'knowhow/habits/tracking.md',
  ];

  it('reads app ids off manifest paths before the apps list loads', () => {
    expect(installedApps(files, { status: 'not-loaded' })).toEqual([
      { id: 'habit-tracker', name: 'habit-tracker' },
      { id: 'streak-widget', name: 'streak-widget' },
    ]);
  });

  it('keeps only listed apps, with their names, once the list loads', () => {
    const app = { id: 'habit-tracker', name: 'Habit Tracker' } as App;
    expect(installedApps(files, { status: 'loaded', data: [app] })).toEqual([
      { id: 'habit-tracker', name: 'Habit Tracker' },
    ]);
  });
});
