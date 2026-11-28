import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { OpenCodeFreeSettings } from '../OpenCodeFreeSettings';

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

describe('OpenCodeFreeSettings', () => {
  const rendered = vnodeToText(OpenCodeFreeSettings());

  it('renders the privacy notice beside the toggle, not behind the explainer', () => {
    expect(rendered).toContain('type="checkbox"');
    expect(rendered).toContain('settings-row-note');
    expect(rendered).toMatch(/third-party relay/);
    expect(rendered).toMatch(/may train on what you send/);
  });

  it('offers no key field, because the tier is keyless', () => {
    expect(rendered).not.toContain('type="password"');
    expect(rendered).not.toContain('settings-text-input');
  });

  it('is off until the preference says otherwise', () => {
    expect(rendered).not.toContain('checked="true"');
  });
});
