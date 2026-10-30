// @vitest-environment jsdom
/** Settings › Access › The machine running Lucidos: the HTTPS row.
 *
 *  Every TLS state renders a true sentence, and no HTTPS always says why:
 *  plain HTTP with no stated reason is the failure this row stops. */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { GatewayTls } from '../../../api/client/control';
import type { Loadable } from '../../../store/types';
import { GatewayHttpsRow, gatewayHttpsSummary } from '../GatewayHttpsRow';

const ORIGIN = 'https://mymac.tailnet-name.ts.net:5252';

function tls(over: Partial<GatewayTls>): GatewayTls {
  return {
    source: 'none',
    name: null,
    expires_at: null,
    error: null,
    operator_serves_other_names: false,
    https_origin: null,
    ...over,
  };
}

let host: HTMLDivElement;

beforeEach(() => {
  host = document.createElement('div');
  document.body.appendChild(host);
});

afterEach(() => {
  render(null, host);
  host.remove();
});

function shown(state: Loadable<GatewayTls>): string {
  act(() => { render(<GatewayHttpsRow tls={state} />, host); });
  return host.textContent ?? '';
}

describe('GatewayHttpsRow', () => {
  it('names the origin a tailnet certificate serves', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ source: 'tailnet', name: 'mymac.tailnet-name.ts.net', https_origin: ORIGIN, expires_at: '2027-01-27T00:00:00Z' }),
    });
    expect(text).toContain('HTTPS: automatic via your tailnet');
    expect(text).toContain(ORIGIN);
    expect(text).toContain('renews on its own');
  });

  it('says a failed update keeps the current certificate serving', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ source: 'tailnet', https_origin: ORIGIN, error: 'tailscaled did not answer' }),
    });
    expect(text).toContain('HTTPS: automatic via your tailnet');
    expect(text).toContain('still serves: tailscaled did not answer');
  });

  it('names an operator certificate', () => {
    expect(shown({ status: 'loaded', data: tls({ source: 'operator' }) })).toContain('your own certificate');
  });

  it('names the tailnet origin an operator certificate covers', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ source: 'operator', name: 'mymac.tailnet-name.ts.net', https_origin: ORIGIN }),
    });
    expect(text).toContain('your own certificate');
    expect(text).toContain(`covers your tailnet name too, so other devices open ${ORIGIN}`);
  });

  it('says why the tailnet name has no HTTPS beside an operator certificate', () => {
    const text = shown({ status: 'loaded', data: tls({ source: 'operator', error: 'MagicDNS is off in this tailnet.' }) });
    expect(text).toContain('Your tailnet name gets no HTTPS: MagicDNS is off in this tailnet.');
  });

  it('says which certificate serves what when both serve', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ source: 'tailnet', name: 'mymac.tailnet-name.ts.net', https_origin: ORIGIN, operator_serves_other_names: true }),
    });
    expect(text).toContain('HTTPS: automatic via your tailnet');
    expect(text).toContain('Every other name gets the certificate LUCIDOS_TLS_CERT names');
  });

  it('says why there is no HTTPS', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ error: 'The tailnet issues no HTTPS certificate for mymac.tailnet-name.ts.net.' }),
    });
    expect(text).toContain('HTTPS: not available');
    expect(text).toContain('The tailnet issues no HTTPS certificate');
  });

  it('never claims plain HTTP for a gateway that listens on this machine only', () => {
    const text = shown({
      status: 'loaded',
      data: tls({ error: 'The gateway listens on this machine only, so it needs no certificate.' }),
    });
    expect(text).toContain('listens on this machine only');
    expect(text).not.toContain('reach this machine over plain HTTP');
  });

  it('shows a failed read as an error, never as "not available"', () => {
    const text = shown({ status: 'failed', error: 'gateway unreachable' });
    expect(text).toContain('gateway unreachable');
    expect(text).not.toContain('not available');
  });

  it('draws nothing before the loader gate opens', () => {
    expect(shown({ status: 'loading' })).toBe('');
  });
});

describe('gatewayHttpsSummary', () => {
  it('never claims HTTPS for a tailnet source with no served origin', () => {
    expect(gatewayHttpsSummary(tls({ source: 'tailnet', error: 'no name' })).title).toBe('HTTPS: not available');
  });
});
