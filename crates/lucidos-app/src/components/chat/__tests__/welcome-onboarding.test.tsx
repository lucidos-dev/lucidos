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
import { WelcomeMessage, ProviderSetupWelcome, SuggestionCarousel, suggestionView } from '../WelcomeMessage';
import { showWelcomeSurface } from '../CreateThreadView';
import { llmConfigured } from '../../../store/store';

type AnyVNode = VNode<Record<string, unknown>>;

/** Whether a vnode subtree contains a vnode of the given component type. Unlike
 *  findByClass this matches function components (used to assert a child
 *  component is rendered without descending into its hook-bearing body). */
function containsComponent(node: ComponentChildren, comp: unknown): boolean {
  if (node === null || node === undefined || typeof node !== 'object') return false;
  if (Array.isArray(node)) return node.some((n) => containsComponent(n, comp));
  const v = node as AnyVNode;
  if (v.type === comp) return true;
  return containsComponent(v.props?.children as ComponentChildren, comp);
}

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

  it('renders the suggestion carousel when a provider is configured', () => {
    llmConfigured.value = true;
    const vnode = WelcomeMessage() as AnyVNode;
    // Configured branch is the inline DOM tree, not the setup component.
    expect(vnode.type).toBe('div');
    // Suggestions render via the chevron carousel (its own component, which makes
    // each suggestion a clickable button that prefills the prompt — covered by
    // e2e/welcome.spec.ts), with the lead-in label above it. The old standalone
    // `welcome-suggestion-chip` markup is gone, and there's no provider-setup CTA.
    expect(containsComponent(vnode, SuggestionCarousel)).toBe(true);
    expect(textOf(vnode)).toContain('A few suggestions');
    expect(findByClass(vnode, 'welcome-suggestion-chip').length).toBe(0);
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
    expect(containsComponent(tree, SuggestionCarousel)).toBe(false);
  });
});

describe('suggestionView — chevron carousel view-model', () => {
  const ideas = ['a', 'b', 'c'];

  it('reports the current item and which chevrons apply', () => {
    expect(suggestionView(ideas, 0)).toEqual({ current: 'a', index: 0, hasPrev: false, hasNext: true });
    expect(suggestionView(ideas, 1)).toEqual({ current: 'b', index: 1, hasPrev: true, hasNext: true });
    expect(suggestionView(ideas, 2)).toEqual({ current: 'c', index: 2, hasPrev: true, hasNext: false });
  });

  it('clamps an out-of-range index into bounds', () => {
    expect(suggestionView(ideas, -5).index).toBe(0);
    expect(suggestionView(ideas, 99).index).toBe(2);
  });
});
