// @vitest-environment jsdom
/**
 * An empty Standing denies list points at the button that fills it.
 *
 * Regression: the shared editor hardcoded its empty hint, so the deny list
 * told the user to press Always allow.
 */
import { describe, it, expect, vi, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

vi.mock('../../../api/client', () => ({
  getAgentDeniedCommands: () => Promise.resolve('# header\n'),
  putAgentDeniedCommands: () => Promise.resolve(),
}));

import { StandingDeniesSection } from '../StandingDeniesSection';

describe('StandingDeniesSection empty state', () => {
  const host = document.createElement('div');

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('names Always deny, not Always allow', async () => {
    document.body.appendChild(host);
    await act(() => render(<StandingDeniesSection />, host));
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)); });

    const hint = host.querySelector('.allowlist-empty')?.textContent ?? '';
    expect(hint).toContain('Always deny');
    expect(hint).not.toContain('Always allow');
  });
});
