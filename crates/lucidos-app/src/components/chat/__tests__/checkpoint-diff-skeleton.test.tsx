// @vitest-environment jsdom
/** The checkpoint modal draws diff cards as its loading placeholder, past the
 *  delay gate, never a spinner. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../api/client')>()),
  getCommandCheckpointDiff: () => new Promise(() => {}),
}));

import { CheckpointDiffModal } from '../CheckpointDiffModal';
import { checkpointDiffModal } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  checkpointDiffModal.value = { checkpoint_id: 'cp-1', summary: 'Edited a file', command: 'sed -i s/a/b/ x' } as never;
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<CheckpointDiffModal />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  checkpointDiffModal.value = null;
  vi.useRealTimers();
});

function modal(): Element {
  return document.querySelector('[data-role="checkpoint-diff-modal"]')!;
}

it('draws no diff skeleton before the gate opens', () => {
  expect(modal().querySelector('.sk-bar')).toBeNull();
});

it('draws diff cards as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  expect(modal().querySelectorAll('.loading-fade-skeleton .diff-view')).toHaveLength(2);
  expect(modal().querySelector('.loading-fade-skeleton .diff-line-content .sk-bar')).not.toBeNull();
  expect(modal().querySelector('.loading-spinner')).toBeNull();
});
