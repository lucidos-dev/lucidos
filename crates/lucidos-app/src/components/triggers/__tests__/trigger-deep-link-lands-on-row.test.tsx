// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { TriggersView } from '../TriggersView';
import { renderMarkdown } from '../../../utils/renderMarkdown';
import { linkifyPaths, _resetLinkifyCacheForTesting } from '../../../utils/linkifyPaths';
import {
  triggers,
  triggerGroups,
  triggerScrollTarget,
  collapsedTriggerSectionIds,
  UNGROUPED_TRIGGER_SECTION_ID,
} from '../../../store/store';
import { clearNavFocus } from '../../shared/focusMarker';
import type { TriggerInfo } from '../../../store/types';


const TRIGGER_ID = '220e6c0d-f626-41c3-955c-6b6a66674ce6';
const CHAT_MARKDOWN =
  `**[Scheduled CI result](trigger:${TRIGGER_ID})** in the [Triggers](triggers) panel.`;

function trigger(id: string, name: string): TriggerInfo {
  return {
    id,
    name,
    slug: id,
    cron_expressions: ['0 9 * * *'],
    timezone: 'UTC',
    paused: false,
    coding_agent_grant: false,
    run: { type: 'intent', intent: 'check CI' },
  };
}

function mountChatPane(): HTMLElement {
  const pane = document.createElement('div');
  pane.className = 'pane pane-thread';
  pane.innerHTML = linkifyPaths(renderMarkdown(CHAT_MARKDOWN), [], []);
  document.body.appendChild(pane);
  return pane;
}

function mountContentPane(): HTMLElement {
  const pane = document.createElement('div');
  pane.className = 'pane pane-content';
  document.body.appendChild(pane);
  return pane;
}

async function waitFor(done: () => boolean, budgetMs = 1000): Promise<void> {
  for (let waited = 0; waited < budgetMs; waited += 20) {
    if (done()) return;
    await new Promise((resolve) => setTimeout(resolve, 20));
  }
}

async function settle(): Promise<void> {
  await new Promise((resolve) => setTimeout(resolve, 200));
}

describe('a trigger deep link lands on the ROW, not on the link pointing at it', () => {
  let contentPane: HTMLElement;
  let scrolled: HTMLElement[];

  beforeEach(() => {
    _resetLinkifyCacheForTesting();
    clearNavFocus();
    document.body.innerHTML = '';
    scrolled = [];
    Element.prototype.scrollIntoView = function scrollIntoView(this: HTMLElement) {
      scrolled.push(this);
    };
    triggers.value = {
      status: 'loaded',
      data: [
        trigger('unrelated-one', 'Morning digest'),
        trigger(TRIGGER_ID, 'Scheduled CI result'),
      ],
    };
    triggerGroups.value = { status: 'loaded', data: [] };
    collapsedTriggerSectionIds.value = new Set();
    mountChatPane();
    contentPane = mountContentPane();
  });

  afterEach(() => {
    render(null, contentPane);
    clearNavFocus();
    triggerScrollTarget.value = null;
    document.body.innerHTML = '';
    vi.restoreAllMocks();
  });

  it('marks the row even though the chat link wears the same attribute', async () => {
    const chatLink = document.querySelector<HTMLElement>('.pane-thread a.trigger-link');
    expect(chatLink?.dataset.triggerId).toBe(TRIGGER_ID);
    expect(document.querySelector(`[data-trigger-id="${TRIGGER_ID}"]`)).toBe(chatLink);
    expect(chatLink!.parentElement!.tagName).toBe('STRONG');

    triggerScrollTarget.value = TRIGGER_ID;
    render(<TriggersView />, contentPane);
    await waitFor(() => triggerScrollTarget.value === null);

    const row = contentPane.querySelector<HTMLElement>(
      `.trigger-row[data-trigger-id="${TRIGGER_ID}"]`,
    );
    expect(row).not.toBeNull();
    expect(chatLink!.classList.contains('nav-focus-stuck')).toBe(false);
    expect(row!.classList.contains('nav-focus-stuck')).toBe(true);
    expect(scrolled).toEqual([row]);
    expect(triggerScrollTarget.value).toBeNull();
  });

  it('opens a collapsed group at once, so the scroll measures the row at rest', async () => {
    const animate = vi.fn(() => ({ cancel() {}, finished: new Promise(() => {}) }));
    (HTMLElement.prototype as unknown as { animate: unknown }).animate = animate;
    try {
      triggerGroups.value = {
        status: 'loaded',
        data: [{ id: 'ci', name: 'CI', order: 0, created: '2026-01-01T00:00:00Z', member_count: 1 }],
      };
      triggers.value = {
        status: 'loaded',
        data: [{ ...trigger(TRIGGER_ID, 'Scheduled CI result'), group_id: 'ci' }],
      };
      collapsedTriggerSectionIds.value = new Set(['ci']);

      triggerScrollTarget.value = TRIGGER_ID;
      render(<TriggersView />, contentPane);
      await waitFor(() => triggerScrollTarget.value === null);

      const row = contentPane.querySelector<HTMLElement>(
        `.trigger-row[data-trigger-id="${TRIGGER_ID}"]`,
      );
      expect(scrolled).toEqual([row]);
      expect(animate).not.toHaveBeenCalled();
    } finally {
      delete (HTMLElement.prototype as unknown as { animate?: unknown }).animate;
    }
  });

  it('opens a collapsed Ungrouped section to land on a row inside it', async () => {
    triggerGroups.value = {
      status: 'loaded',
      data: [{ id: 'ci', name: 'CI', order: 0, created: '2026-01-01T00:00:00Z', member_count: 0 }],
    };
    collapsedTriggerSectionIds.value = new Set([UNGROUPED_TRIGGER_SECTION_ID]);

    triggerScrollTarget.value = TRIGGER_ID;
    render(<TriggersView />, contentPane);
    await waitFor(() => triggerScrollTarget.value === null);

    const row = contentPane.querySelector<HTMLElement>(
      `.trigger-row[data-trigger-id="${TRIGGER_ID}"]`,
    );
    expect(row).not.toBeNull();
    expect(scrolled).toEqual([row]);
    expect(collapsedTriggerSectionIds.value.has(UNGROUPED_TRIGGER_SECTION_ID)).toBe(false);
  });

  it('leaves the target unspent while no panel is mounted', async () => {
    triggerScrollTarget.value = TRIGGER_ID;
    await settle();
    expect(triggerScrollTarget.value).toBe(TRIGGER_ID);
    expect(scrolled).toEqual([]);
  });
});
