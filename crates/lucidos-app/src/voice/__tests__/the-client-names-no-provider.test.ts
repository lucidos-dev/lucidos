import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync, statSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, join, relative, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SRC = resolve(here, '../..');

const VOICE_PATHS = [
  'voice',
  'store/voice.ts',
  'api/client/voice.ts',
  'store/microphones.ts',
  'components/chat/CallToggle.tsx',
  'components/chat/MicrophonePicker.tsx',
];

function sourcesUnder(rel: string): [string, string][] {
  const found: [string, string][] = [];
  const walk = (path: string): void => {
    if (statSync(path).isDirectory()) {
      for (const entry of readdirSync(path)) walk(join(path, entry));
      return;
    }
    if (!/\.tsx?$/.test(path) || /\.test\.tsx?$/.test(path)) return;
    found.push([relative(SRC, path), readFileSync(path, 'utf8')]);
  };
  walk(join(SRC, rel));
  return found;
}

function voiceSources(): [string, string][] {
  return VOICE_PATHS.flatMap(sourcesUnder);
}

function offenders(pattern: RegExp): string[] {
  const hits: string[] = [];
  for (const [rel, text] of voiceSources()) {
    text.split('\n').forEach((line, index) => {
      if (pattern.test(line)) hits.push(`${rel}:${index + 1}`);
    });
  }
  return hits;
}

describe('the voice client names no provider', () => {
  it('holds no provider hostname', () => {
    expect(offenders(/api\.openai\.com|api\.anthropic\.com|openrouter\.ai|api\.x\.ai/i)).toEqual([]);
  });

  it('holds no provider or model name', () => {
    expect(offenders(/openai|anthropic|\bgpt-|\bclaude-|whisper|\bgemini\b|\bgrok\b/i)).toEqual([]);
  });

  it('holds no credential field', () => {
    expect(
      offenders(/\bapi[-_]?key\b|\bapiKey\b|\bBearer\b|\bAuthorization\b|\bclient[-_]?secret\b/),
    ).toEqual([]);
  });

  it('opens its socket through the one route builder, and nowhere else', () => {
    const constructing = voiceSources().filter(([, text]) => /new WebSocket\(/.test(text));
    expect(constructing.map(([rel]) => rel)).toEqual(['voice/ports.ts']);
    expect(constructing[0][1]).toContain('voiceSocketUrl(threadId, ticket)');
  });
});

describe('the voice client writes no audio down', () => {
  it('records nothing', () => {
    expect(offenders(/MediaRecorder|createMediaStreamDestination/)).toEqual([]);
  });

  it('stores nothing', () => {
    expect(offenders(/localStorage|sessionStorage|indexedDB|caches\./)).toEqual([]);
  });

  it('uploads nothing', () => {
    expect(offenders(/\bfetch\(|XMLHttpRequest|navigator\.sendBeacon/)).toEqual([]);
  });
});

describe('the guard is scanning the right tree', () => {
  it('finds every path it claims to cover', () => {
    for (const rel of VOICE_PATHS) {
      expect(sourcesUnder(rel).length, `${rel} matched no source`).toBeGreaterThan(0);
    }
  });

  it('reads enough files to be worth anything', () => {
    expect(voiceSources().length).toBeGreaterThan(5);
  });
});
