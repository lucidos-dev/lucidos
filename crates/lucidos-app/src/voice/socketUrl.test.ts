import { describe, it, expect } from 'vitest';
import { computeVoiceSocketUrl, computeWsEchoUrl, socketScheme } from './socketUrl';

const THREAD = '11111111-2222-3333-4444-555555555555';

describe('the socket scheme', () => {
  it('is secure exactly when the page is', () => {
    expect(socketScheme('https:')).toBe('wss:');
    expect(socketScheme('http:')).toBe('ws:');
  });
});

describe('the call URL', () => {
  it('carries the workspace prefix the gateway routes on', () => {
    const url = computeVoiceSocketUrl({
      api: '/dev/api/v1',
      threadId: THREAD,
      protocol: 'https:',
      host: 'host.example:8443',
    });
    expect(url).toBe(`wss://host.example:8443/dev/api/v1/voice?thread_id=${THREAD}`);
  });

  it('works at a legacy root, where there is no prefix', () => {
    const url = computeVoiceSocketUrl({
      api: '/api/v1',
      threadId: THREAD,
      protocol: 'http:',
      host: 'localhost:3000',
    });
    expect(url).toBe(`ws://localhost:3000/api/v1/voice?thread_id=${THREAD}`);
  });

  it('escapes the thread id rather than pasting it in', () => {
    const url = computeVoiceSocketUrl({
      api: '/api/v1',
      threadId: 'a b&c',
      protocol: 'https:',
      host: 'h',
    });
    expect(url).toBe('wss://h/api/v1/voice?thread_id=a%20b%26c');
  });

  it('names no provider, no model and no credential', () => {
    const url = computeVoiceSocketUrl({
      api: '/dev/api/v1',
      threadId: THREAD,
      protocol: 'https:',
      host: 'h',
    });
    expect(url).not.toMatch(/key|token|model|openai/i);
  });
});

/** The probe must ride the SAME hops the call does, or it answers about a
 *  route nobody dialled. */
describe('the echo URL', () => {
  it('takes the scheme, host and prefix the call takes', () => {
    const origin = { api: '/dev/api/v1', protocol: 'https:', host: 'host.example:8443' };
    expect(computeWsEchoUrl(origin)).toBe('wss://host.example:8443/dev/api/v1/ws-echo');
    expect(computeVoiceSocketUrl({ ...origin, threadId: THREAD })).toContain(
      'wss://host.example:8443/dev/api/v1/',
    );
  });

  it('works at a legacy root, where there is no prefix', () => {
    expect(computeWsEchoUrl({ api: '/api/v1', protocol: 'http:', host: 'localhost:3000' })).toBe(
      'ws://localhost:3000/api/v1/ws-echo',
    );
  });

  it('carries no thread, because it is about the route and not a call', () => {
    expect(computeWsEchoUrl({ api: '/api/v1', protocol: 'https:', host: 'h' })).not.toContain('?');
  });
});
