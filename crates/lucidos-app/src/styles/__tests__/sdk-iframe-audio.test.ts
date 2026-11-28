// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

const here: string = dirname(fileURLToPath(import.meta.url));
const shim: string = readFileSync(
  resolve(here, '../../../../lucidos-agents-api/src/api/sdk_iframe_audio.js'),
  'utf-8',
);

class FakeContext {
  static resumes = 0;
  state = 'suspended';
  destination = {};
  createBuffer(): object { return {}; }
  createBufferSource(): { connect(): void; start(): void } {
    return { connect() {}, start() {} };
  }
  resume(): Promise<void> {
    FakeContext.resumes += 1;
    this.state = 'running';
    return Promise.resolve();
  }
}

const original = (window as unknown as { AudioContext?: unknown }).AudioContext;

afterEach(() => {
  Object.assign(window, { AudioContext: original });
  vi.unstubAllGlobals();
});

describe('the audio unlock shim', () => {
  it('unlocks again on a later gesture once the context is suspended again', () => {
    FakeContext.resumes = 0;
    vi.stubGlobal('Audio', class { volume = 0; src = ''; play() { return Promise.resolve(); } pause() {} });
    Object.assign(window, { AudioContext: FakeContext });
    new Function(shim)();
    const gesture = () => document.dispatchEvent(new Event('click', { bubbles: true }));

    gesture();
    expect(FakeContext.resumes, 'the first gesture unlocks').toBe(1);

    gesture();
    expect(FakeContext.resumes, 'a running context needs no work').toBe(1);

    const ctx = (window as unknown as { _audioCtx: FakeContext })._audioCtx;
    ctx.state = 'suspended';
    gesture();
    expect(FakeContext.resumes, 'a re-suspended context is unlocked again').toBe(2);
  });
});
