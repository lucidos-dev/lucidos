/**
 * The read-only receipts the plugin panels become once a confirm resolves. They
 * are what makes a completed install / uninstall a real page in the content nav
 * history instead of a row that resolves to the bare menu item underneath.
 *
 * The receipts are rendered DIRECTLY rather than through their dispatcher: the
 * VNode walk below stops at function components, because descending into a
 * confirm branch would invoke its render-time hooks outside a real render. The
 * dispatcher's own job (route on the marker) is asserted separately, by which
 * component type it returns. `PluginFileList` is likewise asserted on by its
 * props rather than invoked.
 */
import { describe, it, expect, beforeEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { PluginInstallPanel, PluginInstallReceiptPanel } from '../PluginInstallPanel';
import { PluginUninstallPanel, PluginUninstallReceiptPanel } from '../PluginUninstallPanel';
import { PluginFileList } from '../PluginFileList';
import { panelOverlay } from '../../../store/store';
import type { InlineForm, PluginInstallForm, PluginUninstallForm } from '../../../store/store';

type AnyVNode = VNode<Record<string, unknown>>;

interface Walked {
  /** Every text node, in order. */
  text: string[];
  /** Every intrinsic <button>. */
  buttons: AnyVNode[];
  /** Props of every <PluginFileList>, in order. */
  fileLists: { label: string; files: string[] }[];
  /** Function-component types reached and deliberately not descended into. */
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
    return; // never invoke: a confirm branch's hooks would throw out here
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
        // One of the two staged paths had gone by itself before the confirm
        // ran. A receipt reading off `request.files_present` would claim both
        // were deleted.
        files_deleted: ['apps/habit-tracker/manifest.json'],
        files_missing: ['apps/habit-tracker/index.html'],
      }),
    }));

    expect(acc.fileLists).toEqual([
      { label: 'Deleted (1)', files: ['apps/habit-tracker/manifest.json'] },
      { label: 'Already gone (1)', files: ['apps/habit-tracker/index.html'] },
    ]);
    expect(acc.text.join(' ')).toContain('Removed Habit Tracker');
    expect(acc.text).toContain('Uninstalled');
  });

  it('offers only Close: the files are gone and the staged id is popped', () => {
    const acc = render(PluginUninstallReceiptPanel({
      form: uninstallForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Removed Habit Tracker',
        files_deleted: [],
        files_missing: [],
      }),
    }));

    expect(buttonLabels(acc)).toEqual(['Close']);
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
      { label: 'Files written (1)', files: ['apps/habit-tracker/manifest.json'] },
    ]);
    expect(acc.text.join(' ')).toContain('Installed Habit Tracker');
    expect(acc.text).toContain('Installed');
  });

  it('offers only Close', () => {
    const acc = render(PluginInstallReceiptPanel({
      form: installForm({
        at: '2026-08-06T10:00:00.000Z',
        summary: 'Installed Habit Tracker',
        installed_files: [],
      }),
    }));

    expect(buttonLabels(acc)).toEqual(['Close']);
  });
});
