/**
 * The client speaks exactly the vocabulary the engine speaks, so the two sides
 * must hold the same frames. This test reads the Rust source and fails when a
 * frame or a field is added on one side only.
 *
 * Same shape as `utils/imageBytes.mirror.test.ts`, which reads a Rust table the
 * same way.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import {
  CLIENT_CONTROL_TYPES,
  SERVER_FRAME_PAYLOAD,
  SERVER_FRAME_TYPES,
  type ClientControl,
  type ServerFrame,
} from './frames';
import {
  CALL_REFUSED,
  NO_ROUTE_FOR_A_CALL,
  NO_VOICE_MODEL,
  isSettingsProblem,
} from './refusals';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');
const WIRE_RS = 'crates/lucidos-engine/src/voice/wire.rs';

const source: string = readFileSync(resolve(REPO_ROOT, WIRE_RS), 'utf8');

/** PascalCase to the `snake_case` serde renames it to. */
function snake(name: string): string {
  return name.replace(/(?<!^)([A-Z])/g, '_$1').toLowerCase();
}

/** The body of one `pub enum`, with its doc comments dropped. */
function enumBody(name: string): string {
  const decl = source.indexOf(`pub enum ${name} {`);
  expect(decl, `${name} not found in ${WIRE_RS}`).toBeGreaterThan(-1);
  const end = source.indexOf('\n}', decl);
  return source
    .slice(decl, end)
    .split('\n')
    .filter((line) => !line.trim().startsWith('//'))
    .join('\n');
}

/** Each variant of an enum, as its wire tag and its field names. */
function variants(name: string): Map<string, string[]> {
  const body = enumBody(name);
  const found = new Map<string, string[]>();
  const pattern = /^\s{4}([A-Z][A-Za-z0-9]*)\s*(?:\{([^}]*)\})?\s*,/gm;
  for (const match of body.matchAll(pattern)) {
    const fields = [...(match[2] ?? '').matchAll(/(\w+)\s*:/g)].map((f) => f[1]).sort();
    found.set(snake(match[1]), fields);
  }
  expect(found.size, `no variants parsed out of ${name}`).toBeGreaterThan(0);
  return found;
}

/** The field names of a sample value, which `tsc` checks against the union. */
function keysOf(sample: object): string[] {
  return Object.keys(sample)
    .filter((k) => k !== 'type')
    .sort();
}

/** One of every server frame, so the TS types are read rather than restated. */
const SERVER_SAMPLES: ServerFrame[] = [
  { type: 'session_started', audio: { sample_rate_hz: 24_000, channels: 1, encoding: 'pcm_s16le' } },
  { type: 'user_turn_ended', transcript: '' },
  { type: 'talker_transcript', text: '' },
  { type: 'talker_turn_ended' },
  { type: 'interrupted' },
  { type: 'session_ended', reason: 'hangup' },
  { type: 'error', message: '' },
];

const CLIENT_SAMPLES: ClientControl[] = [{ type: 'barge_in' }, { type: 'hang_up' }];

describe('the client and the engine speak one vocabulary', () => {
  it('reads the tag the way serde writes it', () => {
    // `snake` above stands in for the rename. Drop the attribute and every tag
    // silently becomes PascalCase, which this catches and set equality cannot.
    for (const name of ['ClientControl', 'ServerFrame']) {
      const decl = source.indexOf(`pub enum ${name} {`);
      const attrs = source.slice(Math.max(0, decl - 400), decl);
      expect(attrs, name).toContain('tag = "type", rename_all = "snake_case"');
    }
  });

  it('knows every frame the engine can send, and no other', () => {
    expect([...SERVER_FRAME_TYPES].sort()).toEqual([...variants('ServerFrame').keys()].sort());
  });

  it('says only what the engine will read', () => {
    expect([...CLIENT_CONTROL_TYPES].sort()).toEqual([...variants('ClientControl').keys()].sort());
  });

  it('carries the same fields on every server frame', () => {
    const rust = variants('ServerFrame');
    for (const sample of SERVER_SAMPLES) {
      expect(keysOf(sample), sample.type).toEqual(rust.get(sample.type));
    }
    expect(SERVER_SAMPLES).toHaveLength(rust.size);
  });

  /** The parser's shape table names those same fields, so it is a second copy
   *  and gets the same mirror. A field renamed in Rust would otherwise make
   *  every frame of that type unreadable, silently. */
  it('checks the payload field the engine actually sends', () => {
    const rust = variants('ServerFrame');
    for (const [tag, payload] of Object.entries(SERVER_FRAME_PAYLOAD)) {
      const fields = rust.get(tag);
      expect(fields, tag).toBeDefined();
      expect(payload === null ? [] : [payload[0]], tag).toEqual(fields);
    }
  });

  it('sends a control with no payload, as the engine expects', () => {
    for (const sample of CLIENT_SAMPLES) expect(keysOf(sample)).toEqual([]);
    expect(CLIENT_SAMPLES).toHaveLength(variants('ClientControl').size);
  });

  it('names the same audio spec fields', () => {
    const decl = source.indexOf('pub struct AudioSpec {');
    expect(decl, `AudioSpec not found in ${WIRE_RS}`).toBeGreaterThan(-1);
    const body = source.slice(decl, source.indexOf('\n}', decl));
    const fields = [...body.matchAll(/pub (\w+):/g)].map((m) => m[1]).sort();
    const sample = SERVER_SAMPLES[0];
    expect(sample.type).toBe('session_started');
    if (sample.type !== 'session_started') return;
    expect(Object.keys(sample.audio).sort()).toEqual(fields);
  });
});

/**
 * One engine sentence the client matches on, so it must match exactly.
 *
 * The talker refusal is the only reason a call gives that the reader can act
 * on. So it is the only one that earns a button to Settings. The client
 * recognises it by its words, having nothing else to go on, and a reworded
 * engine would take the button away with nothing failing.
 */
describe('the reason that leads somewhere', () => {
  const VOICE_RS = 'crates/lucidos-engine/src/api/voice.rs';
  const handler: string = readFileSync(resolve(REPO_ROOT, VOICE_RS), 'utf8');

  it('is worded exactly as the engine sends it', () => {
    expect(handler, `${NO_VOICE_MODEL} not found in ${VOICE_RS}`).toContain(NO_VOICE_MODEL);
  });

  it('is the one reason the client offers a way out of', () => {
    expect(isSettingsProblem(NO_VOICE_MODEL)).toBe(true);
    for (const other of [CALL_REFUSED, NO_ROUTE_FOR_A_CALL, 'Something else went wrong.']) {
      expect(isSettingsProblem(other), other).toBe(false);
    }
  });
});

describe('nothing on this wire names a provider', () => {
  it('holds no host, model id or credential field', () => {
    const text = JSON.stringify(SERVER_SAMPLES) + JSON.stringify(CLIENT_SAMPLES);
    expect(text).not.toMatch(/openai|realtime|api[-_]?key|bearer|wss:\/\//i);
  });
});
