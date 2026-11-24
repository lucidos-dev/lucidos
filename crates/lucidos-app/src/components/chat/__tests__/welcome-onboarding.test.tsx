import { Fragment, type ComponentChildren, type VNode } from 'preact';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

vi.mock('../../../store/actions/compose', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/compose')>()),
  startSetupInterview: vi.fn(async () => true),
}));
vi.mock('../../../store/store', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/store')>()),
  showConfirm: vi.fn(async () => true),
}));
vi.mock('../../../store/actions/menu', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/menu')>()),
  openProviderSettings: vi.fn(),
  openFreeProviderSettings: vi.fn(),
}));

import { WelcomeMessage, ProviderSetupWelcome, SetupInterviewWelcome } from '../WelcomeMessage';
import { openFreeProviderSettings, openProviderSettings } from '../../../store/actions/menu';
import { welcomePlacement } from '../CreateThreadView';
import { threadHeaderActions } from '../../layout/ThreadHeaderActions';
import { dialogParagraphs } from '../../shared/DialogMessage';
import { llmConfigured, showConfirm } from '../../../store/store';
import { viewportIsMobile } from '../../../utils/viewport';
import { SETUP_INTERVIEW_PROMPT, startSetupInterview } from '../../../store/actions/compose';

type AnyVNode = VNode<Record<string, unknown>>;

function containsComponent(node: ComponentChildren, comp: unknown): boolean {
  if (node === null || node === undefined || typeof node !== 'object') return false;
  if (Array.isArray(node)) return node.some((n) => containsComponent(n, comp));
  const v = node as AnyVNode;
  if (v.type === comp) return true;
  return containsComponent(v.props?.children as ComponentChildren, comp);
}

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

function textOf(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(textOf).join('');
  const v = node as AnyVNode;
  if (v.type !== Fragment && typeof v.type !== 'string') return '';
  return textOf(v.props.children as ComponentChildren);
}

describe('welcomePlacement: one place, Home first', () => {
  it('belongs to Home whenever Home exists', () => {
    expect(welcomePlacement({ threadsLoaded: true, homeId: 'home', welcomeDismissed: false })).toBe('home');
  });

  it('falls back to the compose view only when there is no Home', () => {
    expect(welcomePlacement({ threadsLoaded: true, homeId: null, welcomeDismissed: false })).toBe('compose');
  });

  it('shows nowhere while the thread list loads, so it never flashes in compose', () => {
    for (const homeId of ['home', null]) {
      expect(welcomePlacement({ threadsLoaded: false, homeId, welcomeDismissed: false })).toBe('none');
    }
  });

  it('shows nowhere once dismissed', () => {
    for (const homeId of ['home', null]) {
      expect(welcomePlacement({ threadsLoaded: true, homeId, welcomeDismissed: true })).toBe('none');
    }
  });
});

describe('WelcomeMessage — provider-aware variant selection', () => {
  afterEach(() => {
    llmConfigured.value = true;
  });

  it('renders the provider-setup variant when no provider is configured', () => {
    llmConfigured.value = false;
    const vnode = WelcomeMessage() as AnyVNode;
    expect(vnode.type).toBe(ProviderSetupWelcome);
  });

  it('renders the setup-interview hero when a provider is configured', () => {
    llmConfigured.value = true;
    const vnode = WelcomeMessage() as AnyVNode;
    expect(vnode.type).toBe('div');
    expect(textOf(vnode)).not.toContain('Or ask me anything');
    expect(findByClass(vnode, 'welcome-carousel').length).toBe(0);
    expect(findByClass(vnode, 'welcome-suggestions-label').length).toBe(0);
    expect(findByClass(vnode, 'welcome-suggestion-chip').length).toBe(0);
    expect(findByClass(vnode, 'welcome-provider-setup').length).toBe(0);
  });
});

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
    expect(vnode.type).toBe(ProviderSetupWelcome);
    expect(containsComponent(vnode, SetupInterviewWelcome)).toBe(false);
    expect(findByClass(ProviderSetupWelcome() as AnyVNode, 'welcome-setup-interview-btn').length).toBe(0);
  });

  it('is a prominent blue button with a hint, matching the provider CTA', () => {
    const tree = SetupInterviewWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-setup-interview-btn');
    expect(btns.length).toBe(1);
    const klass = (btns[0].props.class as string).split(' ');
    expect(klass).toContain('action-btn');
    expect(klass).not.toContain('action-btn-confirm');
    expect(textOf(btns[0])).toContain('Help me get the most out of Lucidos');
    expect(findByClass(tree, 'welcome-setup-interview-hint').length).toBe(1);
  });

  it('offers help beyond work, so the hint does not read as a job interview', () => {
    const hint = textOf(findByClass(SetupInterviewWelcome() as AnyVNode, 'welcome-setup-interview-hint')[0]);
    expect(hint).toContain('at work or outside it');
  });

  it('teaches the durable way back BEFORE the welcome can be dismissed', () => {
    viewportIsMobile.value = false;
    expect(textOf(SetupInterviewWelcome() as AnyVNode)).toContain('again any time from the');
  });

  it('names only the way back that exists on this viewport', () => {
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

  it('is in the thread header, immediately before Search everywhere', () => {
    const keys = threadHeaderActions().map(a => a.key);
    expect(keys).toEqual(['setup-interview', 'search-everywhere']);
  });

  it('is NOT offered when no provider is configured', () => {
    llmConfigured.value = false;
    expect(setupAction()).toBeUndefined();
    expect(threadHeaderActions().map(a => a.key)).toEqual(['search-everywhere']);
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
    const paragraphs = dialogParagraphs(message);
    expect(paragraphs.length).toBe(2);
    expect(paragraphs[1]).toContain('Anything else you need help with?');
    expect(paragraphs[1]).toContain('Just ask in the chat');
  });

  it('sends nothing when the user declines the confirm', async () => {
    vi.mocked(showConfirm).mockResolvedValue(false);
    setupAction()!.onClick!({} as MouseEvent);
    await vi.waitFor(() => expect(showConfirm).toHaveBeenCalledTimes(1));
    expect(startSetupInterview).not.toHaveBeenCalled();
  });
});

describe('setup interview: cross-file wiring', () => {
  const here = dirname(fileURLToPath(import.meta.url));
  const SRC = resolve(here, '../../..');

  it('stays OFF both mobile headers', () => {
    const src: string = readFileSync(resolve(SRC, 'components/layout/MobileAppHeader.tsx'), 'utf8');
    expect(
      src.includes('<ThreadHeaderActions') || src.includes('threadHeaderActions'),
      'no mobile header may render the thread action cluster, which carries the setup interview '
      + '(see the comments at those sites)',
    ).toBe(false);
  });

  it('seeds the exact phrase the engine system prompt routes on', () => {
    expect(SETUP_INTERVIEW_PROMPT.toLowerCase()).toContain('help me get the most out of lucidos');
  });

  it('seeds a sentence that is not narrowed back down to work', () => {
    expect(SETUP_INTERVIEW_PROMPT.toLowerCase()).not.toContain('my work and my week');
  });
});

describe('ProviderSetupWelcome — onboarding content', () => {
  beforeEach(() => {
    vi.mocked(openProviderSettings).mockClear();
    vi.mocked(openFreeProviderSettings).mockClear();
  });

  it('shows the setup CTA pointing at Settings → Providers, and nothing that needs a model', () => {
    const tree = ProviderSetupWelcome() as AnyVNode;
    const btns = findByClass(tree, 'welcome-provider-setup-btn');
    expect(btns.length).toBe(1);
    expect(textOf(btns[0])).toContain('Set up your AI provider');
    expect(textOf(tree)).toContain('Settings → Providers');
    expect(containsComponent(tree, SetupInterviewWelcome)).toBe(false);
  });

  it('names the keyless free tier as the option needing no key', () => {
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
    expect(openProviderSettings).not.toHaveBeenCalled();
  });

  it('leaves the opt-in on the switch, never on this surface', () => {
    const here = dirname(fileURLToPath(import.meta.url));
    const src: string = readFileSync(resolve(here, '../WelcomeMessage.tsx'), 'utf8');
    expect(src).not.toContain('setOpenCodeFreeEnabled');
    expect(src).not.toContain('opencode_free_enabled');
  });
});
