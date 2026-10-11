/**
 * Settings → Models groups the switched-off models behind one disclosure.
 *
 * The grouping is only safe because it hides nothing. A builtin is disable-only
 * and keeps its row forever, so the switch that brings it back must stay
 * reachable. An off `source = 'user'` row must keep its Delete. Those are the
 * two ways a collapsed group turns into a trap, and both are pinned here.
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
    enabled: true,
    created_at: '2026-01-01T00:00:00Z',
    ...over,
  };
}

const OPUS_5 = model({ id: 'claude-opus-5[1m]', label: 'Opus 5 (1M)' });
const OPUS_47 = model({ id: 'claude-opus-4-7', label: 'Opus 4.7', enabled: false });
const OLD_USER = model({ id: 'my/old-model', label: 'My Old Model', source: 'user', enabled: false });

const text = (showOff: boolean, models: ModelInfo[] = [OPUS_5, OPUS_47, OLD_USER]) =>
  vnodeToText(modelManagerList(models, showOff, () => {}));

describe('the Off group', () => {
  it('counts the switched-off models in its header', () => {
    expect(text(false)).toContain('Off (2)');
  });

  it('is absent when every model is on', () => {
    const out = text(false, [OPUS_5]);
    expect(out).not.toContain('Off (');
    expect(out).toContain('Opus 5');
  });

  it('leaves the on models in the flat list', () => {
    expect(text(false)).toContain('Opus 5');
  });

  it('holds the off models back until it is expanded', () => {
    expect(text(false)).not.toContain('Opus 4.7');
    expect(text(true)).toContain('Opus 4.7');
  });

  it('reports its own state, so the chevron and the reader agree', () => {
    expect(text(false)).toContain('aria-expanded="false"');
    expect(text(true)).toContain('aria-expanded="true"');
  });

  it('keeps the switch on an off model, which is the only way back', () => {
    expect(text(true)).toContain('aria-label="Offer Opus 4.7 in the model picker"');
  });

  it('keeps Delete on an off user model', () => {
    expect(text(true)).toContain('Delete');
    // The off builtin beside it must NOT gain one: disable-only is the contract
    // that lets a saved `chat_model` naming a retired model still route.
    expect(text(true).match(/Delete/g)).toHaveLength(1);
  });
});

describe('the vision toggle', () => {
  /** Every row carries it, builtin and user alike, pressed only for a model
   *  that reads images. A wrong or unverified seed is fixed here. */
  it('shows on every row, pressed only when the model reads images', () => {
    const looker = model({ id: 'claude-opus-5-5', label: 'Opus 5.5', vision: true });
    const out = vnodeToText(modelManagerList([looker, OPUS_5], false, () => {}));
    expect(out).toContain('aria-label="Opus 5.5 reads images" data-tooltip="Reads images"');
    expect(out).toMatch(/aria-pressed="true"[^>]*aria-label="Opus 5.5 reads images"/);
    expect(out).toMatch(/aria-pressed="false"[^>]*aria-label="Opus 5 \(1M\) reads images"/);
  });
});
