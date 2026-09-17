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
import {
  VoiceSection,
  TalkerModelExplainer,
  TranscriberModelExplainer,
  TALKER_MODELS,
  TRANSCRIBER_MODELS,
} from '../VoiceSection';
import { preferences } from '../../../store/store';
import {
  DEFAULT_VOICE_TALKER_MODEL,
  DEFAULT_VOICE_TALKER_VOICE,
  DEFAULT_VOICE_TRANSCRIBER_MODEL,
  VOICE_RESIDENT_SECTIONS,
} from '../../../store/actions/preferences';

/** Flatten a vnode tree to text, keeping scalar props. Same shallow walk as
 *  `opencode-free-notice.test.tsx`.
 *
 *  A prop holding a vnode is named and not walked. A row can be handed a whole
 *  explainer, and a walk that dumped it would drown the row it belongs to. Its
 *  contents are read by rendering that component on its own. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown>>;
  const props = (v.props ?? {}) as Record<string, unknown>;
  const scalar = (value: unknown) =>
    typeof value === 'string' || typeof value === 'number' || value === true;
  const isVNode = (value: unknown) =>
    typeof value === 'object' && value !== null && !Array.isArray(value) && 'type' in value;
  const attrs = Object.entries(props)
    .filter(([k]) => k !== 'children')
    .map(([k, value]) => {
      if (scalar(value)) return ` ${k}="${String(value)}"`;
      return isVNode(value) ? ` ${k}="[vnode]"` : '';
    })
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
   *  The two streaming models lead because they are the ones built for a
   *  microphone. The engine branches on `gpt-live-transcribe` to send
   *  `languages` instead of `language`. A typo here offers a row the call
   *  cannot pin a language for. */
  it('offers the streaming transcribers first, without dropping the older models', () => {
    const ids = TRANSCRIBER_MODELS.map((m) => m.value);
    expect(ids.slice(0, 2)).toEqual(['gpt-live-transcribe', 'gpt-realtime-whisper']);
    expect(ids).toContain('gpt-transcribe');
    expect(ids).toEqual(
      expect.arrayContaining(['gpt-4o-mini-transcribe', 'gpt-4o-transcribe', 'whisper-1']),
    );
  });

  /** The picker went stale once: it offered `gpt-realtime`, which left the
   *  provider's catalog, and `gpt-realtime-mini`, which is deprecated. Both are
   *  gone, and the default is the one the picker leads with. */
  it('offers the current realtime family, newest first, led by the default', () => {
    const ids = TALKER_MODELS.map((m) => m.value);
    expect(ids[0]).toBe(DEFAULT_VOICE_TALKER_MODEL);
    expect(ids.slice(0, 4)).toEqual([
      'gpt-realtime-2.1',
      'gpt-realtime-2.1-mini',
      'gpt-realtime-2',
      'gpt-realtime-1.5',
    ]);
  });

  /** Two families in one row, and the engine picks the protocol from the id.
   *  Live is offered and never defaulted: it bills by the minute and holds no
   *  tools, so it is a choice rather than the one a fresh workspace makes. */
  it('offers the Live talker, after the realtime family and never as the default', () => {
    const ids = TALKER_MODELS.map((m) => m.value);
    expect(ids).toContain('gpt-live-1');
    expect(ids.indexOf('gpt-live-1')).toBeGreaterThan(ids.indexOf('gpt-realtime-1.5'));
    expect(DEFAULT_VOICE_TALKER_MODEL.startsWith('gpt-live')).toBe(false);
  });

  /** The two families differ in what a call can do, which no model name says.
   *  The row carries that, since a reader picking a talker is not reading the
   *  section explainer above it.
   *
   *  Rendered on its own, because the section's walk keeps scalar props and an
   *  explainer handed over as a prop is not one. */
  it('explains what a Live call cannot do, on the talker row itself', () => {
    const rendered = vnodeToText(TalkerModelExplainer());
    expect(rendered).toContain('title="Talker model"');
    expect(rendered).toContain('It holds no tools');
    expect(rendered).toContain('bills by the minute');
    expect(rendered).toContain('does not reach the usage rollup');
  });

  /** A Live call settles a question out loud and a permission on the card, and
   *  the row has to say which is which. The copy denied both until ADR 0205,
   *  which is a reader picking Realtime for something Live already does. */
  it('says which half of a card a Live call settles out loud', () => {
    const rendered = vnodeToText(TalkerModelExplainer());
    expect(rendered).toContain('a question it asks you can answer by just saying your answer');
    expect(rendered).toContain('a permission is a tap');
  });

  /** The transcriber row is read by a Realtime call and by nothing else. A
   *  setting with no effect and nothing saying so is the worst kind, and only
   *  the row itself can say it. */
  it('says the transcriber row does nothing on a Live call', () => {
    const rendered = vnodeToText(TranscriberModelExplainer());
    expect(rendered).toContain('title="Transcriber model"');
    expect(rendered).toContain('does nothing on a GPT Live call');
    expect(rendered).toContain('Spoken voice still applies');
  });

  /** The prop reaches both rows, which is what puts the explainers on screen.
   *  A component nothing renders passes the tests above and shows nobody
   *  anything. */
  it('hands each explainer to its row', () => {
    const rendered = render({ voice_enabled: 'true' });
    for (const label of ['Talker model', 'Transcriber model']) {
      expect(rendered).toMatch(
        new RegExp(`<ModelSelectionRow label="${label}"[^>]*explainer="\\[vnode\\]"`),
      );
    }
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
   *  knew its own list would render a model the call is not dialling.
   *
   *  `gpt-realtime-mini` is the case that made this load-bearing: the list
   *  dropped it when the provider deprecated it, and a workspace pinned to it
   *  still dials it. */
  it('keeps a stored talker the curated list does not carry', () => {
    for (const pinned of ['gpt-realtime-next', 'gpt-realtime-mini']) {
      const rendered = render({ voice_enabled: 'true', model_voice_talker: pinned });
      expect(rendered).toContain(`model="${pinned}"`);
    }
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
