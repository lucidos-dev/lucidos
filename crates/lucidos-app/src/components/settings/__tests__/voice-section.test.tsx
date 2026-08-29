/**
 * The voice preferences have a control at all, behind the switch voice runs on.
 *
 * Both model preferences shipped in the engine's catalog with nowhere to set
 * them. So a call that could not find a talker pointed the reader at a Settings
 * page with no such row. This is the test that the rows exist, and that they
 * appear only once the workspace has opted in.
 */
import { describe, it, expect, afterEach } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { VoiceSection } from '../VoiceSection';
import { preferences } from '../../../store/store';
import {
  DEFAULT_VOICE_RESIDENT_SECTIONS,
  DEFAULT_VOICE_TALKER_MODEL,
} from '../../../store/actions/preferences';

/** Flatten a vnode tree to text, keeping scalar props. Same shallow walk as
 *  `opencode-free-notice.test.tsx`. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown>>;
  const props = (v.props ?? {}) as Record<string, unknown>;
  const scalar = (value: unknown) =>
    typeof value === 'string' || typeof value === 'number' || value === true;
  const attrs = Object.entries(props)
    .filter(([k, value]) => k !== 'children' && scalar(value))
    .map(([k, value]) => ` ${k}="${String(value)}"`)
    .join('');
  const tag = typeof v.type === 'string' ? v.type : ((v.type as { name?: string })?.name ?? 'C');
  return `<${tag}${attrs}>${vnodeToText(props.children as ComponentChildren)}</${tag}>`;
}

function render(stored: Record<string, string>): string {
  preferences.value = { status: 'loaded', data: stored };
  return vnodeToText(VoiceSection());
}

describe('the Voice settings section', () => {
  afterEach(() => {
    preferences.value = { status: 'not-loaded' };
  });

  /** Voice is experimental and ships off, so the switch is the whole section
   *  until somebody turns it on. Settings for a feature that is not running
   *  read as settings that do something. */
  it('offers only the switch while voice is off', () => {
    const rendered = render({});
    expect(rendered).toContain('Voice (experimental)');
    expect(rendered).toContain('models:voice-enabled');
    expect(rendered).not.toContain('Talker model');
    expect(rendered).not.toContain('Resident context');
  });

  it('carries both voice preferences once voice is on', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('Talker model');
    expect(rendered).toContain('Resident context');
  });

  /** Placeholder, never value. A field pre-filled with the default reads a
   *  clear as an edit, and saves an empty string on every blur. */
  it('shows each engine default as a placeholder over an empty field', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain(`placeholder="${DEFAULT_VOICE_TALKER_MODEL}"`);
    expect(rendered).toContain(`placeholder="${DEFAULT_VOICE_RESIDENT_SECTIONS}"`);
    expect(rendered).not.toContain(`value="${DEFAULT_VOICE_TALKER_MODEL}"`);
  });

  /** The toast's Open settings button lands on the Models subview, and the
   *  reader has to see this section when it does. */
  it('announces itself under an anchor the rest of Settings can find', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('models:voice');
    expect(rendered).toContain('models:voice-talker');
    expect(rendered).toContain('models:voice-resident-sections');
  });
});
