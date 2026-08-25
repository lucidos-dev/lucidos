import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { OpenCodeFreeSettings } from '../OpenCodeFreeSettings';

/** Flatten a vnode tree to text, keeping scalar props. A COMPONENT vnode keeps
 *  its tag, so the `Explainer` body counts as rendered while its `title` prop
 *  stays visible. Same shallow walk as `mcp-servers-page.test.tsx`. */
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

/** Turning this on sends prompts to a third party with no account and no key.
 *  The terms have to be readable at the switch, not one dialog away, so the
 *  notice and the toggle must render in the same block. */
describe('OpenCodeFreeSettings', () => {
  const rendered = vnodeToText(OpenCodeFreeSettings());

  it('renders the privacy notice beside the toggle, not behind the explainer', () => {
    expect(rendered).toContain('type="checkbox"');
    expect(rendered).toContain('settings-row-note');
    expect(rendered).toMatch(/third-party relay/);
    expect(rendered).toMatch(/may train on what you send/);
  });

  it('offers no key field, because the tier is keyless', () => {
    // The prose may say "no API key"; what must not exist is somewhere to type
    // one, which would imply a credential the relay would reject.
    expect(rendered).not.toContain('type="password"');
    expect(rendered).not.toContain('settings-text-input');
  });

  it('is off until the preference says otherwise', () => {
    // The store is unloaded in a unit test, which is the same answer a fresh
    // workspace gives: absent means off.
    expect(rendered).not.toContain('checked="true"');
  });
});
