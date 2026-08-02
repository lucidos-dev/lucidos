/**
 * The Tailscale section's four states, and the split that produces them.
 *
 * Tailnet state and CLI availability are INDEPENDENT. Conflating them shipped
 * the bug this file exists to prevent: `installed` used to mean "we found a
 * CLI", so a Mac whose CLI could not be located was reported as signed out, and
 * a machine sitting on its tailnet was shown a Sign in button that did nothing
 * (the CLI that was found was the macOS GUI executable, which exits 0 while
 * refusing to work).
 */
import { describe, it, expect } from 'vitest';
import { tailscaleRowState } from '../MobileAccessPage';
import type { TailscaleInfo } from '../../../utils/tauri';

/** A machine with nothing: no app, no CLI, no tailnet. */
const NOTHING: TailscaleInfo = {
  installed: false,
  on_tailnet: false,
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
    // Belt and braces against a detection quirk: telling someone to install
    // Tailscale while they are demonstrably using it is the worst answer
    // available, so tailnet membership always wins.
    expect(tailscaleRowState(onTailnet({ installed: false })).kind).not.toBe('get');
  });

  it('asks for sign-in when installed but off the tailnet, with the CLI driving it', () => {
    expect(tailscaleRowState({ ...NOTHING, installed: true, cli_available: true })).toEqual({
      kind: 'sign-in',
      canRun: true,
    });
  });

  it('still asks for sign-in without a CLI, but cannot run it', () => {
    // The row must still appear (the user does need to sign in); what changes is
    // that we point at the Tailscale app instead of offering a dead button.
    expect(tailscaleRowState({ ...NOTHING, installed: true, cli_available: false })).toEqual({
      kind: 'sign-in',
      canRun: false,
    });
  });

  it('offers Expose on a tailnet that is not serving yet', () => {
    expect(tailscaleRowState(onTailnet({ cli_available: true }))).toEqual({
      kind: 'expose',
      canRun: true,
      magicDnsName: 'mymac.tailnet-name.ts.net',
    });
  });

  it('reports the tailnet correctly with no CLI, and cannot Expose', () => {
    // The regression that started this work: the page must describe a working
    // Tailscale accurately even when it cannot act on it.
    expect(tailscaleRowState(onTailnet({ cli_available: false }))).toEqual({
      kind: 'expose',
      canRun: false,
      magicDnsName: 'mymac.tailnet-name.ts.net',
    });
  });

  it('is on a tailnet even when MagicDNS is disabled', () => {
    // No reverse-lookup name is NOT the same as being offline; gating tailnet
    // membership on the name would report such a tailnet as signed out.
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
    // Same machine, same MagicDNS name, nothing listening: no URL is claimed.
    expect(tailscaleRowState(onTailnet({ cli_available: true })).kind).toBe('expose');
  });
});
