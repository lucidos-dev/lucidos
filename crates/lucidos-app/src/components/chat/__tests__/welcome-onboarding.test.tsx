/**
 * Welcome surface: gating + provider-aware content.
 *
 * Gating is "show until dismissed" (`showWelcomeSurface` = isEmpty &&
 * !welcomeDismissed; the dismissal lives in the DB-backed
 * welcome_suggestions_dismissed preference). Content stays provider-aware: when
 * no LLM provider is configured the welcome guides the user to Settings → Models
 * → Providers instead of offering the setup interview, which would chat into a
 * "no provider" error. These tests invoke the components directly and walk the
 * returned VNode tree (the repo idiom — no DOM render library), and unit-test
 * the pure gating predicate.
 */
import { Fragment, type ComponentChildren, type VNode } from 'preact';
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
// The two onboarding CTAs are deep links. Stub them so a press is observable
// without mounting Settings, and keep the rest of the module real.
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openProviderSettings: vi.fn(),
  openFreeProviderSettings: vi.fn(),
}));

import { WelcomeMessage, ProviderSetupWelcome, SetupInterviewWelcome } from '../WelcomeMessage';
import { openFreeProviderSettings, openProviderSettings } from '../../../store/actions/menu';
import { showWelcomeSurface } from '../CreateThreadView';
import { threadHeaderActions } from '../../layout/ThreadHeaderActions';
import { dialogParagraphs } from '../../shared/DialogMessage';
import { llmConfigured, showConfirm } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
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
 *  descend into function components (matches the ThreadFilterPanel idiom). */
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

/** Plain-text content of a vnode subtree (DOM nodes and fragments). A fragment
 *  renders no DOM of its own but its children are already materialised in
 *  props, so descending it is safe and its text IS on screen; a real function
 *  component is still skipped, since running its body here is what this walker
 *  avoids. */
function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (v.type !== Fragment && typeof v.type !== 'string') return '';
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

  it('renders the setup-interview hero when a provider is configured', () => {
    llmConfigured.value = true;
    const vnode = WelcomeMessage() as AnyVNode;
    // Configured branch is the inline DOM tree, not the setup component.
    expect(vnode.type).toBe('div');
    // The interview is the ONLY action here. The starter suggestions that used
    // to sit under an "Or ask me anything" lead-in are gone (chevron carousel,
    // label and the older `welcome-suggestion-chip` markup alike), so the
    // newcomer has one thing to press. No provider-setup CTA on this branch.
    expect(textOf(vnode)).not.toContain('Or ask me anything');
    expect(findByClass(vnode, 'welcome-carousel').length).toBe(0);
    expect(findByClass(vnode, 'welcome-suggestions-label').length).toBe(0);
    expect(findByClass(vnode, 'welcome-suggestion-chip').length).toBe(0);
    expect(findByClass(vnode, 'welcome-provider-setup').length).toBe(0);
  });
});

/** The first-run entry point into the setup interview. It is the ONLY action on
 *  this surface, and looks like a sibling of the provider-setup CTA. */
describe('SetupInterviewWelcome: first-run entry point', () => {
  beforeEach(() => {
    vi.mocked(startSetupInterview).mockClear();
    llmConfigured.value = true;
    viewportIsMobile.value = false;
  });
  afterEach(() => {
    llmConfigured.value = true;
    viewportIsMobile.value = false;
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

  it('is a prominent blue button with a hint, matching the provider CTA', () => {
    const tree = SetupInterviewWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-setup-interview-btn');
    expect(btns.length).toBe(1);
    // Bare `.action-btn` is the blue default. The green `action-btn-confirm`
    // variant is deliberately NOT on it: green reads as "accept what is already
    // on screen", and this starts something.
    const klass = (btns[0].props.class as string).split(' ');
    expect(klass).toContain('action-btn');
    expect(klass).not.toContain('action-btn-confirm');
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
    // The welcome is dismissible, so the hint has to name a way back while the
    // user can still read it. On desktop that is the header's help button.
    viewportIsMobile.value = false;
    expect(textOf(SetupInterviewWelcome() as AnyVNode)).toContain('again any time from the');
  });

  it('names only the way back that exists on this viewport', () => {
    // Mobile has no help button on either header, so pointing at one would send
    // the user hunting for an icon that is not there. Asking is the way back.
    viewportIsMobile.value = true;
    const hint = textOf(SetupInterviewWelcome() as AnyVNode);
    expect(hint).not.toContain('button, or');
    expect(hint).toContain('any time by just asking me to set you up');
  });

  it('starts the interview on click, with no second gesture', () => {
    const btn = findByClass(SetupInterviewWelcome() as AnyVNode, 'welcome-setup-interview-btn')[0];
    (btn.props.onClick as () => void)();
    expect(startSetupInterview).toHaveBeenCalledTimes(1);
  });
});

/** The durable re-run affordance: a help action immediately left of the New
 *  thread (compose) one in the desktop header, never hidden by the welcome
 *  dismissal. It is DATA rather than a component (see `threadHeaderActions`),
 *  which is what lets it fold into the ⋯ overflow menu as the pane narrows, so
 *  this asserts the spec instead of a rendered button. */
describe('the setup interview action: the durable way back', () => {
  const setupAction = () => threadHeaderActions().find(a => a.key === 'setup-interview');

  beforeEach(() => {
    vi.mocked(startSetupInterview).mockClear();
    vi.mocked(showConfirm).mockClear();
    vi.mocked(showConfirm).mockResolvedValue(true);
    llmConfigured.value = true;
  });
  afterEach(() => {
    llmConfigured.value = true;
  });

  it('is in the thread header, immediately before New thread', () => {
    const keys = threadHeaderActions().map(a => a.key);
    expect(keys).toEqual(['setup-interview', 'new-thread', 'search-everywhere']);
    // Collapse eats from the front, so being first also means this is the first
    // thing to fold into the ⋯ menu when the pane runs out of room, which is
    // right for a once-or-twice action.
  });

  it('is NOT offered when no provider is configured', () => {
    llmConfigured.value = false;
    expect(setupAction()).toBeUndefined();
    // ...and the row is still the other two, in order.
    expect(threadHeaderActions().map(a => a.key)).toEqual(['new-thread', 'search-everywhere']);
  });

  it('confirms before sending, and sends when the user accepts', async () => {
    setupAction()!.onClick!({} as MouseEvent);
    await vi.waitFor(() => expect(startSetupInterview).toHaveBeenCalledTimes(1));
    expect(showConfirm).toHaveBeenCalledTimes(1);
  });

  it('points the reader at the chat for anything else, in its own paragraph', async () => {
    setupAction()!.onClick!({} as MouseEvent);
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
    setupAction()!.onClick!({} as MouseEvent);
    await vi.waitFor(() => expect(showConfirm).toHaveBeenCalledTimes(1));
    // A mis-tap next to New thread must not post into the user's thread.
    expect(startSetupInterview).not.toHaveBeenCalled();
  });
});

/** Source-scan guards. Neither can be asserted by rendering: one is about what a
 *  layout component does NOT render, and the seeded prompt's other half is a
 *  Rust string. */
describe('setup interview: cross-file wiring', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const SRC = resolve(here, '../../..'); // crates/lucidos-app/src

  it('stays OFF both mobile headers', () => {
    // Not an oversight. The interview is a once-or-twice thing, so a permanent
    // icon for it is not worth a slot on a phone header row, and the
    // conversation row could not take one anyway: it centers the brand
    // absolutely and shrink-to-content, so a fourth trailing icon beside
    // compose + search + menu pushes the brand into the cluster at 375px, which
    // e2e/mobile-threads-title-alignment.spec.ts fails on. Pinned here because
    // the natural "make it consistent with desktop" edit is to add it back, and
    // the unit suite would otherwise stay green while the mobile header broke.
    const src: string = readFileSync(resolve(SRC, 'components/layout/MobileAppHeader.tsx'), 'utf8');
    expect(
      src.includes('<ThreadHeaderActions') || src.includes('threadHeaderActions'),
      'no mobile header may render the thread action cluster, which carries the setup interview '
      + '(see the comments at those sites)',
    ).toBe(false);
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
  beforeEach(() => {
    vi.mocked(openProviderSettings).mockClear();
    vi.mocked(openFreeProviderSettings).mockClear();
  });

  it('shows the setup CTA pointing at Settings → Models → Providers, and nothing that needs a model', () => {
    const tree = ProviderSetupWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-provider-setup-btn');
    expect(btns.length).toBe(1);
    expect(textOf(btns[0])).toContain('Set up your AI provider');
    // The fix must steer to provider setup, not offer agent-assuming actions.
    expect(textOf(tree)).toContain('Settings → Models → Providers');
    expect(containsComponent(tree, SetupInterviewWelcome)).toBe(false);
  });

  it('names the keyless free tier as the option needing no key', () => {
    // The credential CTA above it is unreachable without a subscription or a
    // card, and this screen exists for the user who has neither.
    const tree = ProviderSetupWelcome() as AnyVNode;
    const text = textOf(tree);
    expect(text).toContain('OpenCode Free');
    expect(text).toContain('no key');
    const free = findByClass(tree, 'welcome-provider-free-btn');
    expect(free.length).toBe(1);
    expect(textOf(free[0])).toContain('free tier');
  });

  it('routes the free action to the switch and enables nothing', () => {
    const tree = ProviderSetupWelcome() as AnyVNode;
    const free = findByClass(tree, 'welcome-provider-free-btn')[0];
    (free.props.onClick as (e: MouseEvent) => void)({} as MouseEvent);
    expect(openFreeProviderSettings).toHaveBeenCalledTimes(1);
    // Not the section-level link. The tier is one row inside that section, and
    // landing at the top of it is what made the tier hard to find.
    expect(openProviderSettings).not.toHaveBeenCalled();
  });

  it('leaves the opt-in on the switch, never on this surface', () => {
    // ADR 0104: the tier is off by default and states its terms where it is
    // turned on. A welcome that could flip it would move the decision away from
    // the sentence describing it. Source-scanned, because the guarantee is that
    // the setter is not REACHABLE here, not merely that one path avoids it.
    const here = dirname(fileURLToPath(import.meta.url));
    const src: string = readFileSync(resolve(here, '../WelcomeMessage.tsx'), 'utf8');
    expect(src).not.toContain('setOpenCodeFreeEnabled');
    expect(src).not.toContain('opencode_free_enabled');
  });
});
