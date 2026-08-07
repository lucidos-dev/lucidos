/**
 * The restore banner's contract: a success gets out of the way on its own, a
 * failure has to be acknowledged, and the spinner is never the flexing element.
 */

import { describe, it, expect, vi } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import { restoreBanner } from '../RestoreBanner';
import type { GwRestoreStatus } from '../../../api/client/control';

/** Flatten a vnode tree, keeping the markers we assert on. Mirrors
 *  picker-footer.test.tsx. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<Record<string, unknown> & { children?: ComponentChildren }>;
  const tag =
    typeof v.type === 'string' ? v.type : typeof v.type === 'function' ? v.type.name : '';
  const attrs = ['class', 'data-state', 'disabled']
    .filter((k) => v.props?.[k] !== undefined && v.props?.[k] !== false)
    .map((k) => ` ${k}="${String(v.props[k])}"`)
    .join('');
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}${attrs}>${inner}</${tag}>` : inner;
}

const NOOP = () => {};

function render(status: GwRestoreStatus | null, busy = false): string {
  return vnodeToText(restoreBanner({ status, busy, onDismiss: NOOP }));
}

describe('restoreBanner', () => {
  it('renders nothing before the first poll lands, and nothing when idle', () => {
    expect(render(null)).toBe('');
    expect(render({ status: 'idle' })).toBe('');
  });

  it('names the workspace and the phase while running', () => {
    const out = render({ status: 'running', id: 'w1', name: 'personal', phase: 'decrypting' });
    expect(out).toContain('data-state="running"');
    expect(out).toContain('personal');
    expect(out).toContain('Decrypting…');
  });

  it('falls back to the raw phase when the gateway reports an unknown one', () => {
    const out = render({ status: 'running', id: 'w1', name: 'personal', phase: 'reticulating' });
    expect(out).toContain('reticulating');
  });

  it('gives the message its own class, so the spinner is never what flexes', () => {
    // The regression this pins: `> span:first-of-type` matched the SPINNER in
    // the running banner and stretched it into a banner-wide ellipse. The CSS
    // now targets `.ws-picker-restore-text`, which only exists if the message
    // carries it, and the spinner must not.
    const out = render({ status: 'running', id: 'w1', name: 'personal', phase: 'restoring' });
    expect(out).toContain('<span class="ws-picker-restore-spinner">');
    expect(out).toContain('<span class="ws-picker-restore-text">');
    expect(out.indexOf('ws-picker-restore-spinner')).toBeLessThan(
      out.indexOf('ws-picker-restore-text'),
    );
  });

  it('confirms a completed restore with no buttons at all', () => {
    const out = render({ status: 'completed', id: 'w1', name: 'personal' });
    expect(out).toContain('data-state="completed"');
    expect(out).toContain('Restored');
    expect(out).toContain('personal');
    // No Open (the restored row is right above) and no Dismiss (the picker
    // clears the status on a timer instead).
    expect(out).not.toContain('<button');
  });

  it('keeps Dismiss on a failure, so an error cannot vanish unread', () => {
    const out = render({ status: 'failed', name: 'personal', error: 'bad key' });
    expect(out).toContain('data-state="failed"');
    expect(out).toContain('bad key');
    expect(out).toContain('Dismiss');
  });

  it('disables Dismiss while another action is in flight', () => {
    const out = render({ status: 'failed', name: 'personal', error: 'bad key' }, true);
    expect(out).toContain('disabled="true"');
  });

  it('wires Dismiss to the caller', () => {
    const onDismiss = vi.fn();
    const node = restoreBanner({
      status: { status: 'failed', name: 'personal', error: 'bad key' },
      busy: false,
      onDismiss,
    });
    const button = (node as VNode<{ children?: ComponentChildren }>).props
      .children as VNode<{ onClick: () => void }>[];
    button[1].props.onClick();
    expect(onDismiss).toHaveBeenCalledOnce();
  });
});
