/**
 * The agent sandbox bar (ADR 0645): agents cannot start on this machine, or
 * the owner runs them unsandboxed. Components are invoked as plain functions
 * and the vnode tree walked, the repo idiom.
 */
import { describe, expect, it, vi } from 'vitest';
import {
  SANDBOX_BANNER_HEIGHT_VAR,
  agentSandboxBannerBody,
  agentSandboxNotice,
} from '../AgentSandboxBanner';
import { REFUSAL_BANNER_HEIGHT_VAR } from '../WebhookRefusalBanner';
import { findByClass, findByType, textOf } from './vnodeWalk';

const REASON = 'Landlock is not enabled in this kernel';

describe('what the bar says', () => {
  it('says nothing while agents run sandboxed', () => {
    expect(agentSandboxNotice({ unavailable: null, optedOut: false })).toBeNull();
  });

  it('names the reason when agents cannot start', () => {
    const notice = agentSandboxNotice({ unavailable: REASON, optedOut: false });
    expect(notice?.state).toBe('unavailable');
    expect(notice?.detail).toContain(REASON);
  });

  it('warns while the owner runs agents unsandboxed, whatever the machine can do', () => {
    for (const unavailable of [null, REASON]) {
      expect(agentSandboxNotice({ unavailable, optedOut: true })?.state).toBe('owner-opted-out');
    }
  });
});

describe('the bar', () => {
  it('draws the notice and sends the owner to Permissions', () => {
    const onOpenSettings = vi.fn();
    const notice = agentSandboxNotice({ unavailable: REASON, optedOut: false });
    const tree = agentSandboxBannerBody({ layout: 'desktop', notice, onOpenSettings });
    const [bar] = findByClass(tree, 'sandbox-banner');
    expect(bar.props['data-state']).toBe('unavailable');
    expect(textOf(tree)).toContain(REASON);
    const buttons = findByType(tree, 'button');
    expect(buttons).toHaveLength(1);
    (buttons[0].props.onClick as () => void)();
    expect(onOpenSettings).toHaveBeenCalledOnce();
  });

  it('renders nothing without a notice', () => {
    expect(agentSandboxBannerBody({ layout: 'desktop', notice: null, onOpenSettings: () => {} })).toBeNull();
  });

  it('reserves its own height, apart from every other bar', () => {
    expect(SANDBOX_BANNER_HEIGHT_VAR).not.toBe(REFUSAL_BANNER_HEIGHT_VAR);
  });
});
