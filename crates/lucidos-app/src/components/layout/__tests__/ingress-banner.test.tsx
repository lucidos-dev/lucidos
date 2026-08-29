/**
 * The webhook ingress bar: the news that deliveries cannot reach this workspace
 * from outside the machine.
 *
 * Four properties. It shows exactly while an outage stands, with no fuse of its
 * own. It shows ONCE per viewport, the dual-render rule every app-shell banner
 * obeys. It names the ADDRESS FAMILY, which is the whole lesson of the failure
 * it exists to catch. And it borrows nothing from the connection bar beside it,
 * whose vocabulary is about a different thing entirely.
 *
 * Components are invoked as plain functions and the returned vnode tree is
 * walked, the repo idiom. That is why the markup lives in the hook-free
 * `ingressBannerBody` and the gate in `shouldRenderIngressBanner`.
 */
import { describe, expect, it, vi } from 'vitest';
import {
  INGRESS_BANNER_HEIGHT_VAR,
  ingressBannerBody,
  shouldRenderIngressBanner,
} from '../IngressBanner';
import { CONNECTION_BANNER_HEIGHT_VAR } from '../ConnectionBanner';
import { BANNER_HEIGHT_VAR } from '../BackupReminderBanner';
import { webhookIngressNotice } from '../../../utils/webhookIngressNotice';
import { findByClass, findByType, textOf } from './vnodeWalk';
import type { BannerLayout } from '../appBanner';
import type { WebhookIngressOutage } from '../../../api/client';

const DESKTOP = { layout: 'desktop' as BannerLayout, mobileViewport: false };

function outage(over: Partial<WebhookIngressOutage> = {}): WebhookIngressOutage {
  return {
    webhook_name: 'github-ci',
    host: 'node.tailnet.ts.net',
    port: 8443,
    families: ['ipv4'],
    addresses: [],
    down_since: '2026-08-26T22:10:00Z',
    down_secs: 28_800,
    ...over,
  };
}

describe('the bar is up for exactly as long as the outage is', () => {
  it('says nothing while the public path is healthy', () => {
    expect(shouldRenderIngressBanner({ ...DESKTOP, outage: null })).toBe(false);
  });

  it('shows a standing outage immediately, with no fuse of its own', () => {
    // The engine spends two consecutive failed probe cycles before it declares
    // anything. A second delay here would make the user wait twice for one
    // piece of news the engine has already settled.
    expect(shouldRenderIngressBanner({ ...DESKTOP, outage: outage() })).toBe(true);
  });
});

describe('one instance renders, whichever layout is mounted', () => {
  // Both are mounted (the mobile one inside the fixed header, the desktop one
  // in the shell's flow). Rendering both would show two bars and race two
  // ResizeObservers.
  const state = { outage: outage() };

  it('renders only the desktop instance on a desktop viewport', () => {
    expect(shouldRenderIngressBanner({ layout: 'desktop', mobileViewport: false, ...state })).toBe(true);
    expect(shouldRenderIngressBanner({ layout: 'mobile', mobileViewport: false, ...state })).toBe(false);
  });

  it('renders only the mobile instance on a mobile viewport', () => {
    expect(shouldRenderIngressBanner({ layout: 'mobile', mobileViewport: true, ...state })).toBe(true);
    expect(shouldRenderIngressBanner({ layout: 'desktop', mobileViewport: true, ...state })).toBe(false);
  });
});

describe('the three banners never share a height reservation', () => {
  it('each publishes its own property', () => {
    // All three can be up at once, and each measures itself. One shared
    // property would mean whichever measured last wins, and retracting any of
    // them would clear the space the others still need.
    const vars = [INGRESS_BANNER_HEIGHT_VAR, CONNECTION_BANNER_HEIGHT_VAR, BANNER_HEIGHT_VAR];
    expect(new Set(vars).size).toBe(vars.length);
  });
});

describe('ingressBannerBody renders the bar', () => {
  const body = (
    o: WebhookIngressOutage | null = outage(),
    onOpenWebhooks = () => {},
    onDiscuss = () => {},
  ) => ingressBannerBody({ layout: 'desktop', outage: o, onOpenWebhooks, onDiscuss });

  it('states the notice, from the table the Webhooks rows read too', () => {
    const notice = webhookIngressNotice(outage());
    const text = textOf(body());
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('names the address family, so one family down never reads as all', () => {
    // The whole failure this feature catches was IPv4 alone, with IPv6
    // answering correctly all night. A bar that flattened that would repeat the
    // outage at the last layer.
    expect(textOf(body())).toContain('IPv4');
    expect(textOf(body())).not.toContain('IPv6');
    expect(textOf(body(outage({ families: ['ipv4', 'ipv6'] })))).toContain('IPv4 and IPv6');
  });

  it('offers two buttons: one navigates, one starts a conversation', () => {
    const onOpenWebhooks = vi.fn();
    const onDiscuss = vi.fn();
    const buttons = findByType(body(outage(), onOpenWebhooks, onDiscuss), 'button');
    expect(buttons.map((b) => textOf(b))).toEqual(['Discuss', 'Open Webhooks']);

    (buttons[1].props.onClick as () => void)();
    expect(onOpenWebhooks).toHaveBeenCalledTimes(1);
    expect(onDiscuss).not.toHaveBeenCalled();
  });

  it('promises no repair, because the engine performs none', () => {
    // It reports an ingress outage and never re-arms the funnel. A button
    // offering a fix would promise what nothing behind it does.
    for (const button of findByType(body(), 'button')) {
      const label = textOf(button);
      for (const verb of ['Fix', 'Repair', 'Restart', 'Retry', 'Reconnect']) {
        expect(label, `"${verb}" promises work the engine does not do`).not.toContain(verb);
      }
    }
  });

  it('cannot be dismissed', () => {
    // It retracts itself on the next good probe, so a dismiss control would
    // only offer a way to hide a live fault.
    expect(findByClass(body(), 'icon-btn')).toHaveLength(0);
    expect(findByType(body(), 'button')).toHaveLength(2);
  });

  it('borrows no word from the connection light', () => {
    // The .status-dot scale names THIS client's connection. An ingress outage
    // is close to its opposite: a workspace everyone can reach except the
    // senders that matter.
    expect(findByClass(body(), 'status-dot')).toHaveLength(0);
    expect(textOf(body())).not.toContain('offline');
  });

  it('announces politely and carries no left-accent stripe hook', () => {
    // The condition has held for two probe cycles, so it is news rather than an
    // interruption. The stripe is banned outright by
    // .claude/rules/frontend-css.md; the wash is the emphasis.
    const bar = findByClass(body(), 'ingress-banner')[0];
    expect(bar.props.role).toBe('status');
    expect(bar.props.onClick).toBeUndefined();
    expect(bar.props.class as string).not.toContain('accent-edge');
    expect(bar.props.style).toBeUndefined();
  });

  it('renders nothing when there is no outage to report', () => {
    expect(body(null)).toBeNull();
  });
});
