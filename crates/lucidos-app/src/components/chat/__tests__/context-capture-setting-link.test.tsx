// @vitest-environment jsdom
/** A section with no body, on a step that ran with capture off, links to the
 *  Capture context switch. The link closes the viewer, which would otherwise
 *  cover the settings page it opens. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { ContextCapture } from '../../../store/types';
import { contextViewer } from '../../../store/store';

const { openCaptureContextSetting } = vi.hoisted(() => ({ openCaptureContextSetting: vi.fn() }));
vi.mock('../../../store/actions/menu', () => ({ openCaptureContextSetting }));

import { ContextCapturePanel } from '../ContextCapturePanel';

function captureWith(content?: string): ContextCapture {
  return {
    producer: 'main_llm',
    model: 'test-model',
    context_window: 200_000,
    sections: [
      { name: 'System Instructions', budget_delta_chars: 100, role: 'system', content },
      { name: 'Tool Definitions (3)', budget_delta_chars: 50, role: 'system' },
    ],
    tools: [],
    estimated_total_tokens: 40,
    trimmed: false,
  };
}

let host: HTMLDivElement;

function mount(snap: ContextCapture) {
  act(() => { render(<ContextCapturePanel snap={snap} />, host); });
}

function openRow(name: string) {
  const header = [...host.querySelectorAll<HTMLButtonElement>('.context-section-header')]
    .find(b => b.textContent?.includes(name));
  act(() => { header!.click(); });
}

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
  openCaptureContextSetting.mockClear();
});

afterEach(() => {
  render(null, host);
  host.remove();
  contextViewer.value = null;
});

it('links to the switch and closes the viewer when capture was off', () => {
  contextViewer.value = { snapshot: captureWith(), description: 'step' };
  mount(captureWith());
  openRow('Tool Definitions');
  const link = host.querySelector<HTMLButtonElement>('[data-role="capture-context-link"]');
  expect(link).not.toBeNull();
  act(() => { link!.click(); });
  expect(contextViewer.value).toBeNull();
  expect(openCaptureContextSetting).toHaveBeenCalledTimes(1);
});

it('offers no link for a section never recorded while capture was on', () => {
  mount(captureWith('the system prompt'));
  openRow('Tool Definitions');
  expect(host.querySelector('[data-role="capture-context-link"]')).toBeNull();
  expect(host.textContent).toContain("does not record this section's body");
});
