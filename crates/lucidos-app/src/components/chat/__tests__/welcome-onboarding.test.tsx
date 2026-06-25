/**
 * Welcome surface: gating + provider-aware content.
 *
 * Gating is "show until dismissed" (`showWelcomeSurface` = isEmpty &&
 * !welcomeDismissed; the dismissal lives in the DB-backed
 * welcome_suggestions_dismissed preference). Content stays provider-aware: when
 * no LLM provider is configured the welcome guides the user to Settings → Models
 * → Providers instead of offering starter prompts that would chat into a "no
 * provider" error. These tests invoke the components directly and walk the
 * returned VNode tree (the repo idiom — no DOM render library), and unit-test
 * the pure gating predicate.
 */
import type { ComponentChildren, VNode } from 'preact';
import { afterEach, describe, expect, it } from 'vitest';
import { WelcomeMessage, ProviderSetupWelcome } from '../WelcomeMessage';
import { showWelcomeSurface } from '../CreateThreadView';
import { llmConfigured } from '../../../store/store';

type AnyVNode = VNode<Record<string, unknown>>;

/** Collect DOM (string-typed) vnodes whose class list includes `cls`. Does NOT
 *  descend into function components (matches the ThreadFilterDropdown idiom). */
function findByClass(node: ComponentChildren, cls: string): AnyVNode[] {
  if (node === null || node === undefined || typeof node !== 'object') return [];
  if (Array.isArray(node)) return node.flatMap((n) => findByClass(n, cls));
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return [];
  const out: AnyVNode[] = [];
  const klass = (v.props.class as string | undefined) ?? '';
  if (klass.split(' ').includes(cls)) out.push(v);
  return out.concat(findByClass(v.props.children as ComponentChildren, cls));
}

/** Plain-text content of a vnode subtree (DOM nodes only). */
function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

describe('showWelcomeSurface — show-until-dismissed gating', () => {
  it('hides whenever the compose view is not empty', () => {
    expect(showWelcomeSurface({ isEmpty: false, welcomeDismissed: false })).toBe(false);
    expect(showWelcomeSurface({ isEmpty: false, welcomeDismissed: true })).toBe(false);
  });

  it('shows on the empty compose view until dismissed', () => {
    // One rule: show until the user dismisses it (stored in the DB-backed
    // welcome_suggestions_dismissed preference). No workspace-history or
    // provider gating — the provider-aware variant is content, not gating.
    expect(showWelcomeSurface({ isEmpty: true, welcomeDismissed: false })).toBe(true);
    expect(showWelcomeSurface({ isEmpty: true, welcomeDismissed: true })).toBe(false);
  });
});

describe('WelcomeMessage — provider-aware variant selection', () => {
  afterEach(() => {
    llmConfigured.value = true; // restore default for other suites
  });

  it('renders the provider-setup variant when no provider is configured', () => {
    llmConfigured.value = false;
    const vnode = WelcomeMessage() as AnyVNode;
    expect(vnode.type).toBe(ProviderSetupWelcome);
  });

  it('renders the starter-suggestions welcome when a provider is configured', () => {
    llmConfigured.value = true;
    const vnode = WelcomeMessage() as AnyVNode;
    // Configured branch is the inline DOM tree, not the setup component.
    expect(vnode.type).toBe('div');
    expect(findByClass(vnode, 'welcome-suggestion-chip').length).toBeGreaterThan(0);
    expect(findByClass(vnode, 'welcome-provider-setup').length).toBe(0);
  });
});

describe('ProviderSetupWelcome — onboarding content', () => {
  it('shows the setup CTA pointing at Settings → Models → Providers, and no starter prompts', () => {
    const tree = ProviderSetupWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-provider-setup-btn');
    expect(btns.length).toBe(1);
    expect(textOf(btns[0])).toContain('Set up your AI provider');
    // The fix must steer to provider setup, not offer agent-assuming prompts.
    expect(textOf(tree)).toContain('Settings → Models → Providers');
    expect(findByClass(tree, 'welcome-suggestion-chip').length).toBe(0);
  });
});
