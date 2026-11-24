import { describe, it, expect } from 'vitest';
import { tailscaleRowState } from '../MobileAccessPage';
import type { TailscaleInfo } from '../../../utils/tauri';

const NOTHING: TailscaleInfo = {
  installed: false,
  on_tailnet: false,
  reason: null,
  tailnet_ip: null,
  magic_dns_name: null,
  serve_url: null,
  cli_available: false,
};

const onTailnet = (over: Partial<TailscaleInfo> = {}): TailscaleInfo => ({
  ...NOTHING,
  installed: true,
  on_tailnet: true,
  tailnet_ip: '100.64.0.7',
  magic_dns_name: 'mymac.tailnet-name.ts.net',
  ...over,
});

describe('tailscaleRowState', () => {
  it('offers the download only when Tailscale is genuinely absent', () => {
    expect(tailscaleRowState(NOTHING)).toEqual({ kind: 'get' });
  });

  it('never offers the download to a machine that is on a tailnet', () => {
    expect(tailscaleRowState(onTailnet({ installed: false })).kind).not.toBe('get');
  });

  it('asks for sign-in when installed but off the tailnet, with the CLI driving it', () => {
    expect(tailscaleRowState({ ...NOTHING, installed: true, cli_available: true })).toEqual({
      kind: 'sign-in',
      canRun: true,
      reason: null,
    });
  });

  it('still asks for sign-in without a CLI, but cannot run it', () => {
    expect(tailscaleRowState({ ...NOTHING, installed: true, cli_available: false })).toEqual({
      kind: 'sign-in',
      canRun: false,
      reason: null,
    });
  });

  it('carries Tailscale\'s own reason onto the sign-in row', () => {
    const reason = 'Tailscale is installed, but it did not answer.';
    const row = tailscaleRowState({ ...NOTHING, installed: true, reason });
    expect(row).toEqual({ kind: 'sign-in', canRun: false, reason });
  });

  it('offers Expose on a tailnet that is not serving yet', () => {
    expect(tailscaleRowState(onTailnet({ cli_available: true }))).toEqual({
      kind: 'expose',
      canRun: true,
      magicDnsName: 'mymac.tailnet-name.ts.net',
    });
  });

  it('reports the tailnet correctly with no CLI, and cannot Expose', () => {
    expect(tailscaleRowState(onTailnet({ cli_available: false }))).toEqual({
      kind: 'expose',
      canRun: false,
      magicDnsName: 'mymac.tailnet-name.ts.net',
    });
  });

  it('is on a tailnet even when MagicDNS is disabled', () => {
    const row = tailscaleRowState(onTailnet({ magic_dns_name: null }));
    expect(row).toEqual({ kind: 'expose', canRun: false, magicDnsName: null });
  });

  it('shows the HTTPS URL only once serving is proven', () => {
    const serving = onTailnet({ serve_url: 'https://mymac.tailnet-name.ts.net', cli_available: true });
    expect(tailscaleRowState(serving)).toEqual({
      kind: 'serving',
      url: 'https://mymac.tailnet-name.ts.net',
      canRun: true,
    });
    expect(tailscaleRowState(onTailnet({ cli_available: true })).kind).toBe('expose');
  });
});
