/**
 * Settings → Models groups the retired models behind one disclosure.
 *
 * The grouping is only safe because it hides nothing. A retired builtin keeps
 * its row forever, so a saved `chat_model` naming it still routes. A retired
 * `source = 'user'` row must keep its Delete. Both are pinned here. Whether the
 * picker lists a model is the Lucidos Agent's ladder, not this page.
 */
import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { modelManagerList } from '../ModelsManager';
import { Disclosure } from '../../shared/Disclosure';
import type { ModelInfo } from '../../../api/types';

/** Flatten a vnode tree to a string, keeping scalar props. Same shallow walk as
 *  `mcp-servers-page.test.tsx`: the switch carries its accessible name as a
 *  prop, so dropping props would hide what is asserted.
 *
 *  It keeps a `false` prop too, where that copy keeps only `true`. Half of what
 *  this pins is a control reporting itself as CLOSED, and a dropped `false` is
 *  indistinguishable from a missing attribute. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown>>;
  const props = (v.props ?? {}) as Record<string, unknown>;
  // A shut disclosure renders none of its children.
  if (v.type === Disclosure && !props.open) return '';
  const scalar = (value: unknown) =>
    typeof value === 'string' || typeof value === 'number' || typeof value === 'boolean';
  const attrs = Object.entries(props)
    .filter(([k, value]) => k !== 'children' && scalar(value))
    .map(([k, value]) => ` ${k}="${String(value)}"`)
    .join('');
  const tag = typeof v.type === 'string' ? v.type : ((v.type as { name?: string })?.name ?? 'C');
  return `<${tag}${attrs}>${vnodeToText(props.children as ComponentChildren)}</${tag}>`;
}

function model(over: Partial<ModelInfo> & { id: string }): ModelInfo {
  return {
    label: over.id,
    routes: [{ provider: 'vertex', id: over.id, reasoning_efforts: [] }],
    preferred_provider: null,
    vision: false,
    default_effort: null,
    successor: null,
    sort_order: 0,
    source: 'builtin',
    on_chat_ladder: true,
    created_at: '2026-01-01T00:00:00Z',
    ...over,
  };
}

const OPUS_5 = model({ id: 'claude-opus-5[1m]', label: 'Opus 5 (1M)' });
const OPUS_47 = model({ id: 'claude-opus-4-7', label: 'Opus 4.7', successor: 'claude-opus-5-6' });
const OLD_USER = model({ id: 'my/old-model', label: 'My Old Model', source: 'user', successor: 'claude-opus-5-6' });

const text = (showRetired: boolean, models: ModelInfo[] = [OPUS_5, OPUS_47, OLD_USER]) =>
  vnodeToText(modelManagerList(models, showRetired, () => {}));

describe('the Retired group', () => {
  it('counts the retired models in its header', () => {
    expect(text(false)).toContain('Retired (2)');
  });

  it('is absent when every model is current', () => {
    const out = text(false, [OPUS_5]);
    expect(out).not.toContain('Retired (');
    expect(out).toContain('Opus 5');
  });

  it('leaves the current models in the flat list', () => {
    expect(text(false)).toContain('Opus 5');
  });

  it('holds the retired models back until it is expanded', () => {
    expect(text(false)).not.toContain('Opus 4.7');
    expect(text(true)).toContain('Opus 4.7');
  });

  it('reports its own state, so the chevron and the reader agree', () => {
    expect(text(false)).toContain('aria-expanded="false"');
    expect(text(true)).toContain('aria-expanded="true"');
  });

  it('draws no chat ladder switch, which lives on Settings → Intelligence', () => {
    expect(text(true)).not.toContain('chat ladder"');
  });

  it('keeps Delete on a retired user model', () => {
    expect(text(true)).toContain('Delete');
    // The retired builtin beside it must NOT gain one: its row is what lets a
    // saved `chat_model` naming it still route.
    expect(text(true).match(/Delete/g)).toHaveLength(1);
  });
});

describe('the vision toggle', () => {
  /** Every row carries it, builtin and user alike, pressed only for a model
   *  that reads images. A wrong or unverified seed is fixed here. */
  it('shows on every row, pressed only when the model reads images', () => {
    const looker = model({ id: 'claude-opus-5-6', label: 'Opus 5.6', vision: true });
    const out = vnodeToText(modelManagerList([looker, OPUS_5], false, () => {}));
    expect(out).toContain('aria-label="Opus 5.6 reads images" data-tooltip="Reads images"');
    expect(out).toMatch(/aria-pressed="true"[^>]*aria-label="Opus 5.6 reads images"/);
    expect(out).toMatch(/aria-pressed="false"[^>]*aria-label="Opus 5 \(1M\) reads images"/);
  });
});
