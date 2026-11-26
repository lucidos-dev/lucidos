// @vitest-environment jsdom
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';

import { PairingGate } from './PairingGate';

const secret = 'ldp_abcdefghijklmnopqrstuvwxyz';
const rejection = 'That pairing secret is invalid, expired or already used. Create a new pairing code and try again.';
const container = document.createElement('div');
const fetchMock = vi.fn();

beforeEach(async () => {
  document.body.append(container);
  vi.stubGlobal('fetch', fetchMock);
  fetchMock.mockResolvedValueOnce(Response.json({ paired: false }));
  await act(async () => render(<PairingGate><div /></PairingGate>, container));
});

afterEach(() => {
  act(() => render(null, container));
  container.remove();
  vi.unstubAllGlobals();
  fetchMock.mockReset();
});

async function submitSecret(text: string) {
  const input = container.querySelector<HTMLInputElement>('#pairing-secret')!;
  const label = container.querySelector<HTMLInputElement>('#pairing-label')!;
  const button = container.querySelector<HTMLButtonElement>('button[type="submit"]')!;
  await act(async () => {
    input.value = text;
    input.dispatchEvent(new Event('input', { bubbles: true }));
    label.value = 'Test browser';
    label.dispatchEvent(new Event('input', { bubbles: true }));
  });
  expect(button.disabled).toBe(false);
  await act(async () => button.click());
}

it.each([
  ['bare secret', ` ${secret}\n`],
  ['fragment link', `https://example.test/~/#pair=${secret}`],
  ['query link', `https://example.test/~/?pair=${secret}`],
])('redeems a pasted %s against the current gateway', async (_kind, text) => {
  fetchMock.mockResolvedValueOnce(Response.json({ error: rejection }, { status: 400 }));
  await submitSecret(text);
  expect(fetchMock).toHaveBeenCalledTimes(2);
  expect(fetchMock).toHaveBeenLastCalledWith('/~/api/v1/auth/pair', expect.objectContaining({
    method: 'POST',
    body: JSON.stringify({ code: secret, label: 'Test browser' }),
  }));
  await vi.waitFor(() => {
    expect(container.querySelector('[role="alert"]')?.textContent).toBe(rejection);
  });
});

it.each(['12345678', 'https://example.test/~/?pair=12345678'])(
  'leaves the short-code policy to the gateway for %s',
  async (text) => {
    const error = 'A typed code works only on the local network or tailnet.';
    fetchMock.mockResolvedValueOnce(Response.json({ error }, { status: 400 }));
    await submitSecret(text);
    expect(fetchMock).toHaveBeenCalledTimes(2);
    expect(fetchMock).toHaveBeenLastCalledWith('/~/api/v1/auth/pair', expect.objectContaining({
      method: 'POST',
      body: JSON.stringify({ code: '12345678', label: 'Test browser' }),
    }));
    await vi.waitFor(() => {
      expect(container.querySelector('[role="alert"]')?.textContent).toBe(error);
    });
  },
);

it('shows an error without making a redemption request for a malformed secret', async () => {
  await submitSecret('ldp_invalid');
  expect(fetchMock).toHaveBeenCalledTimes(1);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(
    'Paste a valid pairing secret or pairing link.',
  );
});

it('uses the short code when it is entered after an invalid paste', async () => {
  await submitSecret('ldp_invalid');
  const code = container.querySelector<HTMLInputElement>('#pairing-code')!;
  const button = container.querySelector<HTMLButtonElement>('button[type="submit"]')!;
  fetchMock.mockResolvedValueOnce(Response.json({ error: 'Code expired.' }, { status: 400 }));
  await act(async () => {
    code.value = '12345678';
    code.dispatchEvent(new Event('input', { bubbles: true }));
  });
  await act(async () => button.click());
  expect(fetchMock).toHaveBeenLastCalledWith('/~/api/v1/auth/pair', expect.objectContaining({
    method: 'POST',
    body: JSON.stringify({ code: '12345678', label: 'Test browser' }),
  }));
});
