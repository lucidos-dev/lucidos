import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { enginePackaged, preferences } from '../../store/store';
import { getSettingsSearchResults, findSettingsEntry } from './searchIndex';

const originalInnerWidth = window.innerWidth;

function setViewportWidth(px: number): void {
  Object.defineProperty(window, 'innerWidth', { value: px, configurable: true, writable: true });
}

beforeEach(() => {
  // Default bindings (no overrides) — searchEverywhere = mod+Shift+S.
  preferences.value = { status: 'not-loaded' };
  // Default to a dev install; the packaged-only cases opt in explicitly.
  enginePackaged.value = false;
});

afterEach(() => {
  setViewportWidth(originalInnerWidth);
  enginePackaged.value = false;
});

describe('settings search — keyboard shortcuts', () => {
  it('finds a shortcut by its key combo typed with a space ("ctrl shift s")', () => {
    const results = getSettingsSearchResults('ctrl shift s', 20);
    expect(results.some((r) => r.id === 'shortcut:searchEverywhere')).toBe(true);
  });

  it('finds a shortcut by the plus form ("ctrl+shift+w")', () => {
    const results = getSettingsSearchResults('ctrl+shift+w', 20);
    expect(results.some((r) => r.id === 'shortcut:closeThread')).toBe(true);
  });

  it('finds the cheat sheet by name', () => {
    const results = getSettingsSearchResults('keyboard', 20);
    expect(results.some((r) => r.id === 'keyboard-shortcuts')).toBe(true);
  });

  it('resolves a synthesized shortcut entry to the keyboard-shortcuts subview', () => {
    const entry = findSettingsEntry('shortcut:searchEverywhere');
    expect(entry?.subview).toBe('keyboard-shortcuts');
  });

  it('reflects a custom binding in search (rebound search to ctrl+shift+p)', () => {
    preferences.value = { status: 'loaded', data: { keybindings: JSON.stringify({ searchEverywhere: 'mod+shift+p' }) } };
    expect(getSettingsSearchResults('ctrl shift p', 20).some((r) => r.id === 'shortcut:searchEverywhere')).toBe(true);
    // The old default combo no longer matches it.
    expect(getSettingsSearchResults('ctrl shift s', 20).some((r) => r.id === 'shortcut:searchEverywhere')).toBe(false);
  });
});

describe('settings search — Permissions section', () => {
  it('finds the Command safety rows by name', () => {
    const guard = getSettingsSearchResults('command guard', 20);
    expect(guard.some((r) => r.id === 'command-safety:guard')).toBe(true);
    const judge = getSettingsSearchResults('judge model', 20);
    expect(judge.some((r) => r.id === 'command-safety:judge-model')).toBe(true);
  });

  it('finds both allowlist editors by name', () => {
    expect(getSettingsSearchResults('lucidos agent permissions', 20).some((r) => r.id === 'permissions:lucidos')).toBe(true);
    expect(getSettingsSearchResults('claude code permissions', 20).some((r) => r.id === 'permissions:claude-code')).toBe(true);
  });

  it('resolves a Command safety entry to the permissions subview with its anchor', () => {
    const entry = findSettingsEntry('command-safety:guard');
    expect(entry?.subview).toBe('permissions');
    expect(entry?.anchor).toBe('command-safety:guard');
  });

  it('resolves the allowlist editors to their anchors under the permissions subview', () => {
    expect(findSettingsEntry('permissions:lucidos')?.subview).toBe('permissions');
    expect(findSettingsEntry('permissions:lucidos')?.anchor).toBe('permissions:lucidos');
    expect(findSettingsEntry('permissions:claude-code')?.anchor).toBe('permissions:claude-code');
  });
});

describe('settings search — mobile-only rows', () => {
  it('hides the "Keep header visible" mobile row from search on a desktop viewport', () => {
    setViewportWidth(1280);
    const results = getSettingsSearchResults('keep header visible', 20);
    expect(results.some((r) => r.id === 'appearance:mobile-header-sticky')).toBe(false);
    // The "Mobile" section row is mobile-only too.
    expect(getSettingsSearchResults('mobile', 20).some((r) => r.id === 'appearance:mobile')).toBe(false);
  });

  it('surfaces the "Keep header visible" mobile row in search on a mobile viewport', () => {
    setViewportWidth(375);
    const results = getSettingsSearchResults('keep header visible', 20);
    expect(results.some((r) => r.id === 'appearance:mobile-header-sticky')).toBe(true);
  });

  it('keeps the entry resolvable by id regardless of viewport (navigation by recents)', () => {
    setViewportWidth(1280);
    expect(findSettingsEntry('appearance:mobile-header-sticky')?.subview).toBe('appearance');
  });
});

describe('settings search — System section', () => {
  it('finds the System page and its connection details', () => {
    expect(getSettingsSearchResults('system', 20).some((r) => r.id === 'system')).toBe(true);
    expect(getSettingsSearchResults('api url', 20).some((r) => r.id === 'system:connection')).toBe(true);
  });

  it('places Backup, Memory, and Disk Usage under the System breadcrumb', () => {
    expect(findSettingsEntry('backup')?.path).toBe('Settings → System');
    expect(findSettingsEntry('memory')?.path).toBe('Settings → System');
    expect(findSettingsEntry('disk-usage')?.path).toBe('Settings → System');
  });

  it('resolves maintenance to Overview, the page that renders it', () => {
    // The three Overview rows moved with their page when `system` became the
    // submenu. Landing them on the submenu would scroll to nothing.
    const entry = findSettingsEntry('system:maintenance');
    expect(entry?.subview).toBe('system-overview');
    expect(entry?.anchor).toBe('system:maintenance');
  });

  it('never puts the System submenu above the sub-page the user named', () => {
    // The match is a plain substring over label plus keywords, every hit
    // scores 1.0, and results come back in array order. The `system` entry
    // sits above the sub-pages, so any word of theirs in its keywords wins
    // their own query: Enter on the top hit opens a list of ten rows instead
    // of the page. Naming the sub-pages there is what did it.
    for (const [query, id] of [
      ['backup', 'backup'],
      ['memory', 'memory'],
      ['disk usage', 'disk-usage'],
      ['environment variables', 'environment-variables'],
      ['debugging', 'debugging'],
      ['release notices', 'release-notices'],
      // Overview's own vocabulary leads with Overview, the page that holds it.
      ['uptime', 'system-overview'],
    ] as const) {
      expect(getSettingsSearchResults(query, 5)[0]?.id, `"${query}" must lead with ${id}`).toBe(id);
    }
  });
});

describe('settings search: packaged-only rows', () => {
  it('hides the Debugging "Restart engine" row from search on a dev install', () => {
    // Dev keeps its restart in System > Overview as "Rebuild & Restart", so the
    // Debugging row does not render there and must not be offered as a result.
    enginePackaged.value = false;
    expect(getSettingsSearchResults('restart engine', 20).some((r) => r.id === 'debugging:restart-engine')).toBe(false);
  });

  it('surfaces the Debugging "Restart engine" row on a packaged install', () => {
    enginePackaged.value = true;
    expect(getSettingsSearchResults('restart engine', 20).some((r) => r.id === 'debugging:restart-engine')).toBe(true);
  });

  it('keeps the entry resolvable by id regardless of mode (navigation by recents)', () => {
    enginePackaged.value = false;
    const entry = findSettingsEntry('debugging:restart-engine');
    expect(entry?.subview).toBe('debugging');
    expect(entry?.anchor).toBe('debugging:restart-engine');
  });
});

describe('settings search: the Access connect URLs row', () => {
  it('is offered in a plain browser, where the section now renders', () => {
    // It carried `tauriOnly` + `packagedOnly` from when Connect URLs existed
    // only inside the packaged desktop app. The section renders everywhere now,
    // deriving its tailnet rows from two plain-HTTP reads. Gating the search
    // entry hid the address a browser user came looking for, behind the very
    // section showing it.
    enginePackaged.value = false;
    const hits = getSettingsSearchResults('connect url', 20);
    expect(hits.some((r) => r.id === 'access:urls')).toBe(true);
  });

  it('is findable by the word the user is actually after', () => {
    enginePackaged.value = false;
    expect(getSettingsSearchResults('magicdns', 20).some((r) => r.id === 'access:urls')).toBe(true);
    expect(getSettingsSearchResults('tailnet', 20).some((r) => r.id === 'access:urls')).toBe(true);
  });

  it('resolves to the Access subview and its anchor', () => {
    const entry = findSettingsEntry('access:urls');
    expect(entry?.subview).toBe('access');
    expect(entry?.anchor).toBe('access:urls');
  });
});

describe('settings search: the Paired devices row', () => {
  it('resolves to the Devices subview and its anchor', () => {
    // Pairing and push are one row now, so Revoke is found where the device
    // is, not under Access. Two lists under one word were the confusion.
    const entry = findSettingsEntry('devices:paired');
    expect(entry?.subview).toBe('devices');
    expect(entry?.anchor).toBe('devices:list');
  });

  it('is findable by the word the user is actually after', async () => {
    // Gated on the gateway having served the page, which is what `<base href>`
    // says. The suite's DOM stub stamps none, so the entry is hidden by
    // default and this reloads the module with one.
    const stub = document.querySelector;
    (document as unknown as { querySelector: (s: string) => unknown }).querySelector = (s) =>
      s === 'base' ? { getAttribute: () => '/dev/' } : null;
    vi.resetModules();
    try {
      const { getSettingsSearchResults: search } = await import('./searchIndex');
      expect(search('revoke', 20).some((r) => r.id === 'devices:paired')).toBe(true);
      expect(search('paired', 20).some((r) => r.id === 'devices:paired')).toBe(true);
    } finally {
      (document as unknown as { querySelector: unknown }).querySelector = stub;
      vi.resetModules();
    }
  });

  it('is withheld where no row can carry a Revoke button', () => {
    // No gateway served this page, so no device is paired and nothing on the
    // list revokes. A hit landing on nothing is worse than no hit at all.
    expect(getSettingsSearchResults('revoke', 20).some((r) => r.id === 'devices:paired'))
      .toBe(false);
  });
});

describe('settings search: the In-app toasts row', () => {
  it('resolves to the Appearance subview and its anchor', () => {
    const entry = findSettingsEntry('appearance:in-app-toasts');
    expect(entry?.subview).toBe('appearance');
    expect(entry?.anchor).toBe('appearance:in-app-toasts');
  });

  it.each(['toast', 'banner', 'popup', 'distracting'])(
    'is found by "%s", not only by the canonical word',
    (term) => {
      const hit = getSettingsSearchResults(term, 20)
        .some((r) => r.id === 'appearance:in-app-toasts');
      expect(hit).toBe(true);
    },
  );
});
