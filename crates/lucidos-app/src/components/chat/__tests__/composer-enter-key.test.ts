/**
 * What Enter does in the composer, in both modes of `composer_enter_key`.
 *
 * `composerEnterSends` is the composer's only send-on-a-key rule. It runs the
 * real `matchShortcut`, so the default shortcuts are exercised as shipped.
 *
 * Plan: docs/plans/2026-10-16-composer-enter-key-setting.md
 */
import { describe, it, expect, beforeEach } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
import { composerEnterSends } from '../prompt-input-helpers';
import { matchShortcut, KEYBINDINGS_PREF_KEY } from '../../../store/actions/keybindings';
import { preferences } from '../../../store/store';
import { serializeBinding, shortcutDef } from '../../../utils/shortcuts';
import type { ComposerEnterKey } from '../../../store/actions/preferences';
import { PREF_COMPOSER_ENTER_KEY } from '@lucidos/preference-catalog';

const here: string = dirname(fileURLToPath(import.meta.url));
const promptSource = readFileSync(resolve(here, '../PromptInput.tsx'), 'utf-8');

interface Mods {
  shiftKey?: boolean;
  metaKey?: boolean;
  ctrlKey?: boolean;
  altKey?: boolean;
  isComposing?: boolean;
  keyCode?: number;
}

function keydown(key: string, mods: Mods = {}) {
  return {
    key,
    shiftKey: false,
    metaKey: false,
    ctrlKey: false,
    altKey: false,
    isComposing: false,
    keyCode: key === 'Enter' ? 13 : 0,
    ...mods,
  };
}

const BOTH_MODES: readonly ComposerEnterKey[] = PREF_COMPOSER_ENTER_KEY.values;

beforeEach(() => {
  preferences.value = { status: 'loaded', data: {} };
});

describe('mode send (the default): today\'s behaviour', () => {
  it('sends on Enter', () => {
    expect(composerEnterSends(keydown('Enter'), 'send', false)).toBe(true);
  });

  it('leaves Shift+Enter to the browser, which adds a new line', () => {
    expect(composerEnterSends(keydown('Enter', { shiftKey: true }), 'send', false)).toBe(false);
  });

  it('sends on Cmd+Enter and Ctrl+Enter', () => {
    expect(composerEnterSends(keydown('Enter', { metaKey: true }), 'send', false)).toBe(true);
    expect(composerEnterSends(keydown('Enter', { ctrlKey: true }), 'send', false)).toBe(true);
  });
});

describe('mode newline', () => {
  it('leaves plain Enter to the browser, which adds a new line', () => {
    expect(composerEnterSends(keydown('Enter'), 'newline', false)).toBe(false);
  });

  it('sends on Shift+Enter', () => {
    expect(composerEnterSends(keydown('Enter', { shiftKey: true }), 'newline', false)).toBe(true);
  });

  it('sends on Cmd+Enter and Ctrl+Enter', () => {
    expect(composerEnterSends(keydown('Enter', { metaKey: true }), 'newline', false)).toBe(true);
    expect(composerEnterSends(keydown('Enter', { ctrlKey: true }), 'newline', false)).toBe(true);
  });
});

describe.each(BOTH_MODES)('in mode %s', (mode) => {
  it('never sends on an Enter that commits an IME candidate', () => {
    for (const mods of [{}, { shiftKey: true }, { metaKey: true }, { ctrlKey: true }]) {
      expect(composerEnterSends(keydown('Enter', { ...mods, isComposing: true }), mode, false)).toBe(false);
      expect(composerEnterSends(keydown('Enter', { ...mods, keyCode: 229 }), mode, false)).toBe(false);
    }
  });

  it('never sends on the phone layout, where the Send button sends', () => {
    for (const mods of [{}, { shiftKey: true }, { metaKey: true }, { ctrlKey: true }]) {
      expect(composerEnterSends(keydown('Enter', mods), mode, true)).toBe(false);
    }
  });

  it('ignores every key but Enter', () => {
    expect(composerEnterSends(keydown('a'), mode, false)).toBe(false);
    expect(composerEnterSends(keydown('a', { shiftKey: true }), mode, false)).toBe(false);
  });

  it('leaves ⌘⇧↵ to maximizePaneGroup', () => {
    const chord = keydown('Enter', { metaKey: true, shiftKey: true });
    expect(matchShortcut(chord)).toBe('maximizePaneGroup');
    expect(composerEnterSends(chord, mode, false)).toBe(false);
  });

  it('leaves ⌥↵ to askSideQuestion', () => {
    const chord = keydown('Enter', { altKey: true });
    expect(matchShortcut(chord)).toBe('askSideQuestion');
    expect(composerEnterSends(chord, mode, false)).toBe(false);
  });

  it('lets a shortcut the user bound onto Shift+Enter win over sending', () => {
    const shiftEnter = { ...shortcutDef('renameThread').defaultBinding, shift: true, key: 'Enter' };
    preferences.value = {
      status: 'loaded',
      data: { [KEYBINDINGS_PREF_KEY]: JSON.stringify({ renameThread: serializeBinding(shiftEnter) }) },
    };
    const chord = keydown('Enter', { shiftKey: true });
    expect(matchShortcut(chord)).toBe('renameThread');
    expect(composerEnterSends(chord, mode, false)).toBe(false);
  });
});

describe('the composer has one Enter rule', () => {
  it('sends through composerEnterSends with the stored mode', () => {
    expect(promptSource).toMatch(/if \(composerEnterSends\(e, currentComposerEnterKey\(\), isMobile\(\)\)\)/);
  });

  it('keeps no inline Enter-to-send expression of its own', () => {
    expect(promptSource).not.toMatch(/e\.key === 'Enter' && !e\.shiftKey/);
    expect(promptSource).not.toMatch(/matchShortcut\(/);
  });
});
