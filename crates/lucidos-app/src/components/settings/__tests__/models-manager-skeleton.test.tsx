// @vitest-environment jsdom
/** Settings › Models draws model rows while the registry loads, past the delay
 *  gate. */
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ModelsManager } from '../ModelsManager';
import { chatModels } from '../../../store/store';
import { SPINNER_DELAY_MS } from '../../../hooks/useDelayedLoading';

let host: HTMLDivElement;

beforeEach(() => {
  vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] });
  chatModels.value = { status: 'loading' };
  host = document.createElement('div');
  document.body.appendChild(host);
  act(() => { render(<ModelsManager />, host); });
});

afterEach(() => {
  render(null, host);
  host.remove();
  chatModels.value = { status: 'not-loaded' };
  vi.useRealTimers();
});

it('draws no skeleton before the gate opens', () => {
  expect(host.querySelector('.sk-bar')).toBeNull();
});

it('draws model rows as the skeleton once the gate opens', () => {
  act(() => { vi.advanceTimersByTime(SPINNER_DELAY_MS); });
  const rows = host.querySelectorAll('.loading-fade-skeleton .model-manager-row');
  expect(rows.length).toBeGreaterThan(0);
  expect(rows[0]!.querySelector('.model-manager-name .sk-bar')).not.toBeNull();
  expect(rows[0]!.querySelector('input')).toBeNull();
});
