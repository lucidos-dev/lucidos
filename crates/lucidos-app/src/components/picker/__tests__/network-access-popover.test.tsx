import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { networkAccessBody, type NetworkEditor } from '../NetworkAccessPopover';
import type { Loadable } from '../../../store/types';
import { draftFromBind } from '../../../utils/bindMode';

/** Flatten a vnode tree into a string with `class` / `data-state` preserved so
 *  we can assert on per-state markers. Mirrors directory-picker-loadable.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{
    children?: ComponentChildren;
    class?: string;
    ['data-state']?: string;
  }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const cls = v.props?.class ? ` class="${v.props.class}"` : '';
  const state = v.props?.['data-state'] ? ` data-state="${v.props['data-state']}"` : '';
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${cls}${state}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};

function editor(bind: string, inherit = true, detected: string | null = null): NetworkEditor {
  return {
    config: { gateway_bind: bind, inherit, detected_tailscale_ip: detected },
    draft: draftFromBind(bind, inherit),
  };
}

function render(state: Loadable<NetworkEditor>, saving = false, busy = false) {
  return vnodeToText(
    networkAccessBody({
      state,
      saving,
      busy,
      onMode: NOOP,
      onAddress: NOOP,
      onInherit: NOOP,
      onFillDetected: NOOP,
      onRetry: NOOP,
      onCancel: NOOP,
      onSave: NOOP,
    }),
  );
}

/** The active segment, or null when nothing is marked active. */
function activeMode(text: string): string | null {
  return text.match(/<button class="ws-picker-net-mode active">([^<]*)<\/button>/)?.[1] ?? null;
}

describe('networkAccessBody: it opens on the saved bind', () => {
  it('marks NO mode active until the saved config lands', () => {
    // The regression: with the draft held in signals that outlived the config,
    // reopening rendered the last CLICKED mode as active while the refetch was
    // still in flight, then snapped to the saved one. An unsettled state must
    // claim nothing.
    for (const state of [
      { status: 'not-loaded' },
      { status: 'loading' },
      { status: 'failed', error: 'boom' },
    ] as Loadable<NetworkEditor>[]) {
      const text = render(state);
      expect(activeMode(text)).toBeNull();
      expect(text).not.toContain('ws-picker-net-mode active');
    }
  });

  it('marks exactly the saved mode active once settled', () => {
    expect(activeMode(render({ status: 'loaded', data: editor('all') }))).toBe('All interfaces');
    expect(activeMode(render({ status: 'loaded', data: editor('loopback') }))).toBe(
      'Loopback only',
    );
    expect(activeMode(render({ status: 'loaded', data: editor('100.101.71.58') }))).toBe(
      'Tailnet / IP',
    );
  });

  it('reports the load phase so the controls dim + go inert until settled', () => {
    expect(render({ status: 'loading' })).toContain('data-state="loading"');
    expect(render({ status: 'not-loaded' })).toContain('data-state="loading"');
    expect(render({ status: 'failed', error: 'boom' })).toContain('data-state="failed"');
    expect(render({ status: 'loaded', data: editor('all') })).toContain('data-state="ready"');
  });
});

describe('networkAccessBody: no layout jumps', () => {
  it('keeps the address row mounted in every state, opening it only for address mode', () => {
    // Mounted always (so the settle animates rather than snapping), but only
    // `is-open` when the saved/edited mode actually uses it.
    for (const state of [
      { status: 'loading' },
      { status: 'loaded', data: editor('all') },
      { status: 'loaded', data: editor('100.101.71.58') },
    ] as Loadable<NetworkEditor>[]) {
      expect(render(state)).toContain('ws-picker-net-collapse');
    }
    expect(render({ status: 'loading' })).not.toContain('ws-picker-net-collapse is-open');
    expect(render({ status: 'loaded', data: editor('all') })).not.toContain(
      'ws-picker-net-collapse is-open',
    );
    expect(render({ status: 'loaded', data: editor('100.101.71.58') })).toContain(
      'ws-picker-net-collapse is-open',
    );
  });

  it('opens the failure row only on failure, and keeps it mounted otherwise', () => {
    const failed = render({ status: 'failed', error: 'gateway unreachable' });
    expect(failed).toContain('ws-picker-net-retry');
    expect(failed).toContain('gateway unreachable');
    expect(failed).toContain('ws-picker-net-collapse is-open');
    // Loaded keeps the row mounted (collapsed) so recovering from a failure
    // animates shut instead of popping.
    const ok = render({ status: 'loaded', data: editor('all') });
    expect(ok).toContain('ws-picker-net-retry');
    expect(ok).not.toContain('ws-picker-net-collapse is-open');
  });
});

describe('networkAccessBody: save is explained, never mutely grey', () => {
  it('says there is nothing to save when the draft still matches what is stored', () => {
    // The user read an OFFERED Save as proof the shown bind was not theirs.
    // An untouched draft must say so instead of leaving them to infer it.
    expect(render({ status: 'loaded', data: editor('all') })).toContain('No changes to save');
  });

  it('says what is missing when address mode has no usable address', () => {
    // The case that stranded the user: picking Tailnet with no detected IP left
    // an empty field and a grey Save with no stated reason.
    const empty: NetworkEditor = {
      config: { gateway_bind: 'all', inherit: true, detected_tailscale_ip: null },
      draft: { mode: 'address', address: '', inherit: true },
    };
    expect(render({ status: 'loaded', data: empty })).toContain('Enter an IP address');
  });

  it('offers Save with no reason line once the draft is a valid change', () => {
    const changed: NetworkEditor = {
      config: { gateway_bind: 'all', inherit: true, detected_tailscale_ip: null },
      draft: { mode: 'address', address: '100.101.71.58', inherit: true },
    };
    const text = render({ status: 'loaded', data: changed });
    expect(text).not.toContain('ws-picker-net-blocked');
    expect(text).toContain('>Save<');
  });

  it('states no reason while unsettled, since the dimmed controls already say that', () => {
    expect(render({ status: 'loading' })).not.toContain('ws-picker-net-blocked');
    expect(render({ status: 'failed', error: 'boom' })).not.toContain('ws-picker-net-blocked');
  });

  it('drops the reason while saving, so it cannot contradict the Saving label', () => {
    const text = render({ status: 'loaded', data: editor('all') }, true);
    expect(text).toContain('Saving…');
    expect(text).not.toContain('ws-picker-net-blocked');
  });
});

describe('networkAccessBody: save affordance', () => {
  it('shows the saving label while a write is in flight', () => {
    expect(render({ status: 'loaded', data: editor('all') }, true)).toContain('Saving…');
    expect(render({ status: 'loaded', data: editor('all') }, false)).toContain('>Save<');
  });

  it('offers the detected Tailscale address as click-to-fill when there is one', () => {
    const withIp = render({
      status: 'loaded',
      data: editor('100.101.71.58', true, '100.101.71.58'),
    });
    expect(withIp).toContain('ws-picker-net-detected');
    expect(withIp).toContain('Detected Tailscale');
    // No detection: the generic hint stands in, same row, same height.
    const noIp = render({ status: 'loaded', data: editor('100.101.71.58') });
    expect(noIp).not.toContain('ws-picker-net-detected');
    expect(noIp).toContain('Your Tailscale 100.x address');
  });
});
