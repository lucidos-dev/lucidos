// @vitest-environment jsdom
/**
 * The event row's fold rolls open through `<Disclosure>`, like every other
 * fold in the app.
 */
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

import { EventRowFoldView } from '../EventRow';

let host: HTMLElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

const toggle = () => host.querySelector<HTMLButtonElement>('.event-row-fold-toggle')!;
const body = () => host.querySelector('.disclosure > .disclosure-body > .event-row-fold-body');

describe('EventRowFoldView', () => {
  it('starts folded and unfolds the body inside a Disclosure', async () => {
    await act(() => {
      render(<EventRowFoldView label="Summary" body="Cleaned up." />, host);
    });
    expect(host.querySelector('details')).toBeNull();
    expect(toggle().textContent).toBe('Summary');
    expect(toggle().getAttribute('aria-expanded')).toBe('false');
    expect(body()).toBeNull();

    await act(() => toggle().click());
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    expect(body()?.textContent).toBe('Cleaned up.');
  });

  it('starts unfolded when asked to', async () => {
    await act(() => {
      render(<EventRowFoldView label="Error" open body="merge conflict" />, host);
    });
    expect(toggle().getAttribute('aria-expanded')).toBe('true');
    expect(body()?.textContent).toBe('merge conflict');
  });

  it('gives machine data a scrolling pre', async () => {
    await act(() => {
      render(<EventRowFoldView label="Details" pre open body="{}" />, host);
    });
    expect(host.querySelector('.disclosure-body > pre.event-row-fold-pre')?.textContent).toBe('{}');
  });
});
