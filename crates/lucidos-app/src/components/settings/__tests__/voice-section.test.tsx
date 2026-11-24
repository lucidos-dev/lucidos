import { describe, it, expect, afterEach, vi } from 'vitest';
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

const reach = vi.hoisted(() => ({ reachable: true }));
vi.mock('../../../voice/socketUrl', async (original) => ({
  ...(await original<typeof import('../../../voice/socketUrl')>()),
  voiceReachable: () => reach.reachable,
}));

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
    reach.reachable = true;
  });

  it('explains, with no switch, that a relayed workspace has no voice', () => {
    reach.reachable = false;
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('opened through another gateway');
    expect(rendered).toContain('models:voice"');
    expect(rendered).not.toContain('models:voice-enabled');
    expect(rendered).not.toContain('Talker model');
  });

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

  it('shows the engine defaults for the transcriber and the spoken voice', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain(`model="${DEFAULT_VOICE_TRANSCRIBER_MODEL}"`);
    expect(rendered).toContain(`value="${DEFAULT_VOICE_TALKER_VOICE}"`);
  });

  it('offers the streaming transcribers first, then the default, and no retired model', () => {
    const ids = TRANSCRIBER_MODELS.map((m) => m.value);
    expect(ids).toEqual(['gpt-live-transcribe', 'gpt-realtime-whisper', 'gpt-transcribe']);
    expect(ids).toContain(DEFAULT_VOICE_TRANSCRIBER_MODEL);
  });

  it('offers the current realtime family, newest first, led by the default', () => {
    const ids = TALKER_MODELS.map((m) => m.value);
    expect(ids[0]).toBe(DEFAULT_VOICE_TALKER_MODEL);
    expect(ids.slice(1, 4)).toEqual([
      'gpt-realtime-2.1-mini',
      'gpt-realtime-2',
      'gpt-realtime-1.5',
    ]);
  });

  it('offers the Live talker, after the realtime family and never as the default', () => {
    const ids = TALKER_MODELS.map((m) => m.value);
    expect(ids).toContain('gpt-live-1');
    expect(ids.indexOf('gpt-live-1')).toBeGreaterThan(ids.indexOf('gpt-realtime-1.5'));
    expect(DEFAULT_VOICE_TALKER_MODEL.startsWith('gpt-live')).toBe(false);
  });

  it('explains what a Live call cannot do, on the talker row itself', () => {
    const rendered = vnodeToText(TalkerModelExplainer());
    expect(rendered).toContain('title="Talker model"');
    expect(rendered).toContain('It holds no tools');
    expect(rendered).toContain('bills by the minute');
    expect(rendered).toContain('does not reach the usage rollup');
  });

  it('says which half of a card a Live call settles out loud', () => {
    const rendered = vnodeToText(TalkerModelExplainer());
    expect(rendered).toContain('a question it asks you can answer by just saying your answer');
    expect(rendered).toContain('a permission is a tap');
  });

  it('says the transcriber row does nothing on a Live call', () => {
    const rendered = vnodeToText(TranscriberModelExplainer());
    expect(rendered).toContain('title="Transcriber model"');
    expect(rendered).toContain('does nothing on a GPT Live call');
    expect(rendered).toContain('Spoken voice still applies');
  });

  it('hands each explainer to its row', () => {
    const rendered = render({ voice_enabled: 'true' });
    for (const label of ['Talker model', 'Transcriber model']) {
      expect(rendered).toMatch(
        new RegExp(`<ModelSelectionRow label="${label}"[^>]*explainer="\\[vnode\\]"`),
      );
    }
  });

  it('keeps the transcriber row a closed list and the spoken voice free', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('models:voice-transcriber');
    expect(rendered.match(/freeText="true"/g)).toHaveLength(1);
    expect(rendered).toMatch(/models:voice-talker-voice[\s\S]*freeText="true"/);
  });

  it('offers a toggle per resident section, on by default', () => {
    const rendered = render({ voice_enabled: 'true' });
    for (const section of VOICE_RESIDENT_SECTIONS) {
      expect(rendered).toContain(`aria-label="${section.title}"`);
    }
    expect(rendered).not.toContain('who-and-where,this-thread');
  });

  it('follows the stored list rather than the defaults', () => {
    const rendered = render({
      voice_enabled: 'true',
      voice_resident_sections: 'this-thread',
    });
    const on = /aria-label="This conversation" checked="true"/;
    const off = /aria-label="What this workspace has" checked="true"/;
    expect(rendered).toMatch(on);
    expect(rendered).not.toMatch(off);
  });

  it('leaves every toggle off when the stored list is empty', () => {
    const rendered = render({ voice_enabled: 'true', voice_resident_sections: '' });
    for (const section of VOICE_RESIDENT_SECTIONS) {
      expect(rendered).toContain(`aria-label="${section.title}"`);
      expect(rendered).not.toMatch(new RegExp(`aria-label="${section.title}" checked`));
    }
  });

  it('shows the engine default as the picked talker while nothing is stored', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain(`model="${DEFAULT_VOICE_TALKER_MODEL}"`);
  });

  it('keeps a stored talker the curated list does not carry', () => {
    for (const pinned of ['gpt-realtime-next', 'gpt-realtime-2.2-preview']) {
      const rendered = render({ voice_enabled: 'true', model_voice_talker: pinned });
      expect(rendered).toContain(`model="${pinned}"`);
    }
  });

  it('announces itself under an anchor the rest of Settings can find', () => {
    const rendered = render({ voice_enabled: 'true' });
    expect(rendered).toContain('models:voice');
    expect(rendered).toContain('models:voice-talker');
    expect(rendered).toContain('models:voice-resident-sections');
  });
});
