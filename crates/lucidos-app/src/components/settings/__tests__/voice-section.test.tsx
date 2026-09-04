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
import { VoiceSection, TRANSCRIBER_MODELS } from '../VoiceSection';
import { preferences } from '../../../store/store';
import {
  DEFAULT_VOICE_TALKER_MODEL,
  DEFAULT_VOICE_TALKER_VOICE,
  DEFAULT_VOICE_TRANSCRIBER_MODEL,
  VOICE_RESIDENT_SECTIONS,
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

  it('carries every voice preference once voice is on', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('Talker model');
    expect(rendered).toContain('Transcriber model');
    expect(rendered).toContain('Spoken voice');
    expect(rendered).toContain('Resident context');
  });

  /** The second and last model in the loop had no row at all. So a workspace
   *  could not see what was transcribing it, let alone change it. */
  it('shows the engine defaults for the transcriber and the spoken voice', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain(`model="${DEFAULT_VOICE_TRANSCRIBER_MODEL}"`);
    expect(rendered).toContain(`value="${DEFAULT_VOICE_TALKER_VOICE}"`);
  });

  /** The list is read rather than rendered: `vnodeToText` keeps scalar props,
   *  so the array never reaches the string above.
   *
   *  Live transcription leads because it is the one built for a microphone. The
   *  engine branches on that same id to send `languages` instead of `language`.
   *  A typo here offers a row the call cannot pin a language for. */
  it('offers live transcription first, without dropping the older models', () => {
    const ids = TRANSCRIBER_MODELS.map((m) => m.value);
    expect(ids[0]).toBe('gpt-live-transcribe');
    expect(ids).toContain('gpt-transcribe');
    expect(ids).toEqual(
      expect.arrayContaining(['gpt-4o-mini-transcribe', 'gpt-4o-transcribe', 'whisper-1']),
    );
  });

  /** The list stays curated, which the user asked for explicitly. Free text
   *  belongs to the Spoken voice row, whose names the provider owns. */
  it('keeps the transcriber row a closed list and the spoken voice free', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('models:voice-transcriber');
    expect(rendered.match(/freeText="true"/g)).toHaveLength(1);
    expect(rendered).toMatch(/models:voice-talker-voice[\s\S]*freeText="true"/);
  });

  /** A toggle per section, rather than a field of comma-separated ids. The
   *  engine owns the registry and this list mirrors it. */
  it('offers a toggle per resident section, on by default', () => {
    const rendered = render({ voice_enabled: 'true' });
    for (const section of VOICE_RESIDENT_SECTIONS) {
      expect(rendered).toContain(`aria-label="${section.title}"`);
    }
    expect(rendered).not.toContain('who-and-where,this-thread');
  });

  /** A stored list is what the toggles read, and a section left out of it is
   *  off however it ships. */
  it('follows the stored list rather than the defaults', () => {
    const rendered = render({
      voice_enabled: 'true',
      voice_resident_sections: 'this-thread',
    });
    const on = /aria-label="This conversation so far" checked="true"/;
    const off = /aria-label="What this workspace has" checked="true"/;
    expect(rendered).toMatch(on);
    expect(rendered).not.toMatch(off);
  });

  /** Turning every section off has to stay off. An empty stored value used to
   *  read as "never set", which brought all three back. */
  it('leaves every toggle off when the stored list is empty', () => {
    const rendered = render({ voice_enabled: 'true', voice_resident_sections: '' });
    for (const section of VOICE_RESIDENT_SECTIONS) {
      expect(rendered).toContain(`aria-label="${section.title}"`);
      expect(rendered).not.toMatch(new RegExp(`aria-label="${section.title}" checked`));
    }
  });

  /** The opposite rule for the picker, and the reason is the same one: it must
   *  show what a call dials, and it has no placeholder to say it with. */
  it('shows the engine default as the picked talker while nothing is stored', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain(`model="${DEFAULT_VOICE_TALKER_MODEL}"`);
  });

  /** The agent can write any id through `set_preference`. A picker that only
   *  knew its own list would render a model the call is not dialling. */
  it('keeps a stored talker the curated list does not carry', () => {
    const rendered = render({ voice_enabled: 'true', model_voice_talker: 'gpt-realtime-next' });
    expect(rendered).toContain('model="gpt-realtime-next"');
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
