import { describe, expect, it, vi } from 'vitest';
import {
  REFUSAL_BANNER_HEIGHT_VAR,
  otherRefusalsPhrase,
  refusalBannerBody,
  shouldRenderRefusalBanner,
} from '../WebhookRefusalBanner';
import { INGRESS_BANNER_HEIGHT_VAR, ingressBannerBody } from '../IngressBanner';
import { CONNECTION_BANNER_HEIGHT_VAR } from '../ConnectionBanner';
import { BANNER_HEIGHT_VAR } from '../BackupReminderBanner';
import { webhookRefusalNotice } from '../../../utils/webhookRefusalNotice';
import { findByClass, findByType, textOf } from './vnodeWalk';
import type { BannerLayout } from '../appBanner';
import type { WebhookRefusal } from '../../../api/client';

const DESKTOP = { layout: 'desktop' as BannerLayout, mobileViewport: false };

function refusal(over: Partial<WebhookRefusal> = {}): WebhookRefusal {
  return {
    webhook_id: '6f1c0f3e-0000-4000-8000-000000000001',
    webhook_name: 'GitHub workflow runs',
    enabled: false,
    cause: 'disabled',
    refusals: 42,
    reasons: { disabled: 42 },
    refusing_since: '2026-09-02T04:41:06Z',
    refusing_secs: 1_555_200,
    ...over,
  };
}

describe('the bar is up for exactly as long as the fault is', () => {
  it('says nothing while every hook is landing what it gets', () => {
    expect(shouldRenderRefusalBanner({ ...DESKTOP, refusal: null })).toBe(false);
  });

  it('shows a standing refusal immediately, with no fuse of its own', () => {
    expect(shouldRenderRefusalBanner({ ...DESKTOP, refusal: refusal() })).toBe(true);
  });
});

describe('one instance renders, whichever layout is mounted', () => {
  const state = { refusal: refusal() };

  it('renders only the desktop instance on a desktop viewport', () => {
    expect(shouldRenderRefusalBanner({ layout: 'desktop', mobileViewport: false, ...state })).toBe(true);
    expect(shouldRenderRefusalBanner({ layout: 'mobile', mobileViewport: false, ...state })).toBe(false);
  });

  it('renders only the mobile instance on a mobile viewport', () => {
    expect(shouldRenderRefusalBanner({ layout: 'mobile', mobileViewport: true, ...state })).toBe(true);
    expect(shouldRenderRefusalBanner({ layout: 'desktop', mobileViewport: true, ...state })).toBe(false);
  });
});

describe('the four banners never share a height reservation', () => {
  it('each publishes its own property', () => {
    const vars = [
      REFUSAL_BANNER_HEIGHT_VAR,
      INGRESS_BANNER_HEIGHT_VAR,
      CONNECTION_BANNER_HEIGHT_VAR,
      BANNER_HEIGHT_VAR,
    ];
    expect(new Set(vars).size).toBe(vars.length);
  });
});

describe('otherRefusalsPhrase', () => {
  it('says nothing for the ordinary case of one', () => {
    expect(otherRefusalsPhrase([refusal()])).toBeNull();
    expect(otherRefusalsPhrase([])).toBeNull();
  });

  it('counts the rest rather than naming them', () => {
    expect(otherRefusalsPhrase([refusal(), refusal()])).toBe('1 other webhook is too.');
    expect(otherRefusalsPhrase([refusal(), refusal(), refusal()]))
      .toBe('2 other webhooks are too.');
  });
});

describe('refusalBannerBody renders the bar', () => {
  const body = (
    r: WebhookRefusal | null = refusal(),
    others: string | null = null,
    onOpenWebhooks = () => {},
    onDiscuss = () => {},
  ) => refusalBannerBody({ layout: 'desktop', refusal: r, others, onOpenWebhooks, onDiscuss });

  it('states the notice, from the table the Webhooks row reads too', () => {
    const notice = webhookRefusalNotice(refusal());
    const text = textOf(body());
    expect(text).toContain(notice.title);
    expect(text).toContain(notice.detail);
  });

  it('never sends a switched-off hook to the signature', () => {
    const text = textOf(body());
    expect(text).toContain('switched off');
    expect(text).toContain('Nothing is wrong with the signature or the secret');
  });

  it('carries the cause on the element, so a future rule can read it', () => {
    expect(findByClass(body(), 'refusal-banner')[0].props['data-cause']).toBe('disabled');
    const verifying = refusal({ cause: 'verification', enabled: true });
    expect(findByClass(body(verifying), 'refusal-banner')[0].props['data-cause'])
      .toBe('verification');
  });

  it('adds the count of the hooks it is not speaking for', () => {
    expect(textOf(body(refusal(), '2 other webhooks are too.')))
      .toContain('2 other webhooks are too.');
  });

  it('offers two buttons: one navigates, one starts a conversation', () => {
    const onOpenWebhooks = vi.fn();
    const onDiscuss = vi.fn();
    const buttons = findByType(body(refusal(), null, onOpenWebhooks, onDiscuss), 'button');
    expect(buttons.map((b) => textOf(b))).toEqual(['Discuss', 'Open Webhooks']);

    (buttons[1].props.onClick as () => void)();
    expect(onOpenWebhooks).toHaveBeenCalledTimes(1);
    expect(onDiscuss).not.toHaveBeenCalled();
  });

  it('offers the same row the ingress bar does, in the same order', () => {
    const ingress = ingressBannerBody({
      layout: 'desktop',
      outage: {
        webhook_name: 'github-ci',
        host: 'node.tailnet.ts.net',
        port: 8443,
        families: ['ipv4'],
        addresses: [],
        down_since: '2026-08-26T22:10:00Z',
        down_secs: 28_800,
      },
      onOpenWebhooks: () => {},
      onDiscuss: () => {},
    });
    expect(findByType(body(), 'button').map((b) => textOf(b)))
      .toEqual(findByType(ingress, 'button').map((b) => textOf(b)));
  });

  it('fires Discuss from the Discuss button, for either cause', () => {
    for (const r of [refusal(), refusal({ cause: 'verification', enabled: true })]) {
      const onOpenWebhooks = vi.fn();
      const onDiscuss = vi.fn();
      const buttons = findByType(body(r, null, onOpenWebhooks, onDiscuss), 'button');
      (buttons[0].props.onClick as () => void)();
      expect(onDiscuss).toHaveBeenCalledTimes(1);
      expect(onOpenWebhooks).not.toHaveBeenCalled();
    }
  });

  it('promises no repair, because the engine performs none', () => {
    for (const button of findByType(body(), 'button')) {
      const label = textOf(button);
      for (const verb of ['Fix', 'Repair', 'Enable', 'Retry', 'Reconnect']) {
        expect(label, `"${verb}" promises work the engine does not do`).not.toContain(verb);
      }
    }
  });

  it('cannot be dismissed', () => {
    expect(findByClass(body(), 'icon-btn')).toHaveLength(0);
    expect(findByType(body(), 'button')).toHaveLength(2);
  });

  it('borrows no word from the connection light', () => {
    expect(findByClass(body(), 'status-dot')).toHaveLength(0);
    expect(textOf(body())).not.toContain('offline');
  });

  it('announces politely and carries no left-accent stripe hook', () => {
    const bar = findByClass(body(), 'refusal-banner')[0];
    expect(bar.props.role).toBe('status');
    expect(bar.props.onClick).toBeUndefined();
    expect(bar.props.class as string).not.toContain('accent-edge');
    expect(bar.props.style).toBeUndefined();
  });

  it('renders nothing when there is nothing to report', () => {
    expect(body(null)).toBeNull();
  });
});
