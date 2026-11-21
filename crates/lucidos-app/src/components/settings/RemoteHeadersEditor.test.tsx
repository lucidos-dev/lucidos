// @vitest-environment jsdom
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { RemoteHeadersEditor, headersFromDraft } from './RemoteHeadersEditor';

let host: HTMLDivElement;
beforeEach(() => { host = document.createElement('div'); document.body.append(host); });
afterEach(() => { render(null, host); host.remove(); });

describe('remote proxy headers', () => {
  it('rejects duplicate names rather than silently overwriting a secret', () => {
    expect(() => headersFromDraft([{ name: 'Token', value: 'a' }, { name: 'token', value: 'b' }])).toThrow(/duplicate/i);
    expect(headersFromDraft([{ name: ' Token ', value: ' secret ' }])).toEqual({ token: ' secret ' });
    expect(() => headersFromDraft([{ name: '', value: 'secret' }])).toThrow(/name/i);
  });

  it('rolls open and masks values, with add and remove controls', async () => {
    const change = vi.fn();
    await act(async () => { render(<RemoteHeadersEditor rows={[{ name: 'token', value: 'secret' }]} onChange={change} />, host); });
    const toggle = host.querySelector('button')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('false');
    await act(async () => { toggle.click(); });
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    const value = host.querySelector<HTMLInputElement>('input[type=password]')!;
    expect(value.value).toBe('secret');
    expect(value.autocomplete).toBe('off');
    expect(host.textContent).toContain('The packaged macOS app uses Keychain; other installations use an owner-only key file.');
    await act(async () => { Array.from(host.querySelectorAll('button')).find(b => b.textContent === 'Remove header')!.click(); });
    expect(change).toHaveBeenCalledWith([]);
  });
});
