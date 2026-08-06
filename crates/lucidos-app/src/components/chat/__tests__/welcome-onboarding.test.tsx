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
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

// The setup-interview entry points SEND rather than prefill, so every click test
// here would otherwise fire a real compose send. Stub the two actions and keep
// the rest of the module real.
vi.mock('../../../store/actions/compose', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/compose')>()),
  startSetupInterview: vi.fn(async () => true),
}));
// `showConfirm` resolves a Promise off a signal the real modal drives, which
// never renders here. Override just that export; `llmConfigured` must stay the
// SAME signal instance the components read.
vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showConfirm: vi.fn(async () => true),
}));

import { WelcomeMessage, ProviderSetupWelcome, SuggestionCarousel, SetupInterviewWelcome, suggestionView } from '../WelcomeMessage';
import { showWelcomeSurface } from '../CreateThreadView';
import { SetupInterviewButton } from '../../shared/SetupInterviewButton';
import { dialogParagraphs } from '../../shared/DialogMessage';
import { llmConfigured, showConfirm } from '../../../store/store';
import { SETUP_INTERVIEW_PROMPT, startSetupInterview } from '../../../store/actions/compose';

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
    expect(textOf(vnode)).toContain('Or ask me anything');
    expect(findByClass(vnode, 'welcome-suggestion-chip').length).toBe(0);
    expect(findByClass(vnode, 'welcome-provider-setup').length).toBe(0);
  });
});

/** The first-run entry point into the setup interview. It is the primary action
 *  on this surface, so it must render above the carousel and look like a
 *  sibling of the provider-setup CTA, not like a sixth suggestion. */
describe('SetupInterviewWelcome: first-run entry point', () => {
  beforeEach(() => {
    vi.mocked(startSetupInterview).mockClear();
    llmConfigured.value = true;
  });
  afterEach(() => {
    llmConfigured.value = true;
  });

  it('renders on the normal first-run state', () => {
    const vnode = WelcomeMessage() as AnyVNode;
    expect(containsComponent(vnode, SetupInterviewWelcome)).toBe(true);
  });

  it('does NOT render when no provider is configured', () => {
    llmConfigured.value = false;
    const vnode = WelcomeMessage() as AnyVNode;
    // Provider setup takes the whole surface: an interview that cannot reach a
    // model is worse than no button.
    expect(vnode.type).toBe(ProviderSetupWelcome);
    expect(containsComponent(vnode, SetupInterviewWelcome)).toBe(false);
    expect(findByClass(ProviderSetupWelcome() as AnyVNode, 'welcome-setup-interview-btn').length).toBe(0);
  });

  it('is a prominent confirm-styled button with a hint, matching the provider CTA', () => {
    const tree = SetupInterviewWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-setup-interview-btn');
    expect(btns.length).toBe(1);
    // Additive variant: the base class must ride along or it renders as a plain
    // grey browser button.
    const klass = btns[0].props.class as string;
    expect(klass.split(' ')).toEqual(expect.arrayContaining(['action-btn', 'action-btn-confirm']));
    expect(textOf(btns[0])).toContain('Help me get the most out of Lucidos');
    expect(findByClass(tree, 'welcome-setup-interview-hint').length).toBe(1);
  });

  it('offers help beyond work, so the hint does not read as a job interview', () => {
    // The interview's rung 1 asks which parts of their life to cover, and work
    // is one option rather than the assumption. A newcomer who came about
    // training or the household has to be able to tell from this hint alone
    // that they are in the right place.
    const hint = textOf(findByClass(SetupInterviewWelcome() as AnyVNode, 'welcome-setup-interview-hint')[0]);
    expect(hint).toContain('at work or outside it');
  });

  it('teaches the durable way back BEFORE the welcome can be dismissed', () => {
    // The welcome is dismissible, so the hint has to name the header affordance
    // while the user can still read it.
    expect(textOf(SetupInterviewWelcome() as AnyVNode)).toContain('again any time from the');
  });

  it('starts the interview on click, with no second gesture', () => {
    const btn = findByClass(SetupInterviewWelcome() as AnyVNode, 'welcome-setup-interview-btn')[0];
    (btn.props.onClick as () => void)();
    expect(startSetupInterview).toHaveBeenCalledTimes(1);
  });
});

/** The durable re-run affordance: a help button immediately left of the New
 *  thread (compose) icon in every header, never hidden by the welcome dismissal. */
describe('SetupInterviewButton: the durable way back', () => {
  beforeEach(() => {
    vi.mocked(startSetupInterview).mockClear();
    vi.mocked(showConfirm).mockClear();
    vi.mocked(showConfirm).mockResolvedValue(true);
    llmConfigured.value = true;
  });
  afterEach(() => {
    llmConfigured.value = true;
  });

  it('renders as a header icon button', () => {
    const btn = SetupInterviewButton({}) as AnyVNode;
    expect(btn.type).toBe('button');
    expect((btn.props.class as string).split(' ')).toEqual(
      expect.arrayContaining(['icon-btn', 'header-icon']),
    );
    expect(btn.props['data-role']).toBe('setup-interview-toggle');
  });

  it('does NOT render when no provider is configured', () => {
    llmConfigured.value = false;
    expect(SetupInterviewButton({})).toBe(null);
  });

  it('confirms before sending, and sends when the user accepts', async () => {
    const btn = SetupInterviewButton({}) as AnyVNode;
    (btn.props.onClick as () => void)();
    await vi.waitFor(() => expect(startSetupInterview).toHaveBeenCalledTimes(1));
    expect(showConfirm).toHaveBeenCalledTimes(1);
  });

  it('points the reader at the chat for anything else, in its own paragraph', async () => {
    const btn = SetupInterviewButton({}) as AnyVNode;
    (btn.props.onClick as () => void)();
    await vi.waitFor(() => expect(showConfirm).toHaveBeenCalledTimes(1));
    const [message] = vi.mocked(showConfirm).mock.calls[0];
    // The blank line is what makes `DialogMessage` render it as a second
    // paragraph rather than running it onto the interview description.
    const paragraphs = dialogParagraphs(message);
    expect(paragraphs.length).toBe(2);
    expect(paragraphs[1]).toContain('Anything else you need help with?');
    expect(paragraphs[1]).toContain('Just ask in the chat');
  });

  it('sends nothing when the user declines the confirm', async () => {
    vi.mocked(showConfirm).mockResolvedValue(false);
    const btn = SetupInterviewButton({}) as AnyVNode;
    (btn.props.onClick as () => void)();
    await vi.waitFor(() => expect(showConfirm).toHaveBeenCalledTimes(1));
    // A mis-tap next to New thread must not post into the user's thread.
    expect(startSetupInterview).not.toHaveBeenCalled();
  });
});

/** Source-scan guards. Neither can be asserted by rendering: the header sites
 *  live in hook-heavy layout components, and the seeded prompt's other half is
 *  a Rust string. */
describe('setup interview: cross-file wiring', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

  it('sits immediately left of the compose icon on the two headers with room', () => {
    // Desktop, and the mobile Threads header. Both put the button directly left
    // of New thread.
    for (const rel of ['components/layout/AppHeader.tsx', 'components/layout/MobileAppHeader.tsx']) {
      const src: string = readFileSync(resolve(SRC, rel), 'utf8');
      const first = src.indexOf('<SetupInterviewButton');
      expect(first, `${rel}: no <SetupInterviewButton /> at all`).toBeGreaterThan(-1);
      const compose = src.indexOf('brand-compose-btn', first);
      expect(compose, `${rel}: no compose button after the setup-interview button`).toBeGreaterThan(-1);
      // "Immediately left" means nothing else renders between the two.
      const gap = src.slice(first, compose);
      expect(
        gap.split('\n').filter((l: string) => l.includes('<button') || l.includes('<Search')).length,
        `${rel}: <SetupInterviewButton /> is not adjacent to the compose button`,
      ).toBeLessThanOrEqual(1);
    }
  });

  it('stays OFF the mobile conversation header, which has no room for it', () => {
    // Not an oversight, a measured constraint. That row centers the brand
    // absolutely and shrink-to-content, so a fourth trailing icon beside
    // compose + search + menu pushes the brand into the cluster at 375px, which
    // e2e/mobile-threads-title-alignment.spec.ts fails on. Pinned here because
    // the natural "make it consistent" edit is to add it back, and the unit
    // suite would otherwise stay green while the mobile header broke.
    const src: string = readFileSync(resolve(SRC, 'components/layout/MobileAppHeader.tsx'), 'utf8');
    const header = src.slice(src.indexOf('function MobileThreadHeader'));
    expect(
      header.includes('<SetupInterviewButton'),
      'MobileThreadHeader must not render <SetupInterviewButton /> (see the comment at that site)',
    ).toBe(false);
    // The mobile Threads header, which DOES have room, must still carry it.
    expect(src.slice(0, src.indexOf('function MobileThreadHeader'))).toContain('<SetupInterviewButton');
  });

  it('seeds the exact phrase the engine system prompt routes on', () => {
    // `SETUP_INTERVIEW_RULE` in
    // crates/lucidos-engine/src/engine/chat/process/system_prompt.rs keys the
    // load_knowhow route on this clause. Its Rust-side twin,
    // `setup_interview_route_matches_the_frontend_seeded_prompt`, reads
    // compose.ts and asserts the same thing from the other direction, so the
    // pair cannot drift apart in either edit order.
    expect(SETUP_INTERVIEW_PROMPT.toLowerCase()).toContain('help me get the most out of lucidos');
  });

  it('seeds a sentence that is not narrowed back down to work', () => {
    // The interview covers personal admin, training and learning on the same
    // footing as a job (system-knowhow/setup-interview.md, rung 1). This is the
    // one sentence the user watches themselves send, so it must not re-narrow
    // the scope the knowhow just widened.
    expect(SETUP_INTERVIEW_PROMPT.toLowerCase()).not.toContain('my work and my week');
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
