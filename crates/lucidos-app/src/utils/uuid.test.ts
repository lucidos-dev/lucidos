import { describe, it, expect, afterEach } from 'vitest';
import { generateUuid } from './uuid';

const V4_RE = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;

// Emulate an insecure origin, where crypto exists but randomUUID does not.
function removeRandomUUID() {
  Object.defineProperty(crypto, 'randomUUID', { value: undefined, configurable: true });
}

describe('generateUuid', () => {
  const original = crypto.randomUUID;
  afterEach(() => {
    Object.defineProperty(crypto, 'randomUUID', { value: original, configurable: true });
  });

  it('returns a v4 UUID via crypto.randomUUID when available', () => {
    expect(generateUuid()).toMatch(V4_RE);
  });

  it('falls back to getRandomValues when randomUUID is missing (insecure origin)', () => {
    removeRandomUUID();
    expect(generateUuid()).toMatch(V4_RE);
  });

  it('fallback produces unique values', () => {
    removeRandomUUID();
    const seen = new Set(Array.from({ length: 200 }, () => generateUuid()));
    expect(seen.size).toBe(200);
  });
});
