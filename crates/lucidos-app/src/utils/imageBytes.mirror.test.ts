import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { fileURLToPath } from 'node:url';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { dirname, resolve } from 'node:path';
import {
  ALLOWED_IMAGE_MIMES,
  HEIC_BRANDS,
  UNSUPPORTED_IMAGE_FORMATS,
  describeUnsupportedImage,
} from './imageBytes';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../..');
const BLOBS_RS = 'crates/lucidos-core/src/core/blobs.rs';

const source: string = readFileSync(resolve(REPO_ROOT, BLOBS_RS), 'utf8');

function pairTable(name: string): [string, string][] {
  const decl = source.indexOf(`const ${name}: &[(&str, &str)] = &[`);
  expect(decl, `${name} not found in ${BLOBS_RS}`).toBeGreaterThan(-1);
  const end = source.indexOf('];', decl);
  const body = source.slice(decl, end);
  return [...body.matchAll(/\("([^"]+)",\s*"([^"]+)"\)/g)].map((m) => [m[1], m[2]]);
}

function heicBrands(): string[] {
  const fn = source.indexOf('pub fn sniff_image_mime');
  expect(fn, `sniff_image_mime not found in ${BLOBS_RS}`).toBeGreaterThan(-1);
  const macro = source.indexOf('matches!(', fn);
  const end = source.indexOf(') {', macro);
  const body = source.slice(macro, end);
  return [...body.matchAll(/b"([a-z0-9]{4})"/g)].map((m) => m[1]);
}

describe('the client accepts exactly what the engine stores', () => {
  it('mirrors ALLOWED_IMAGE_MIME_EXT', () => {
    const rust = pairTable('ALLOWED_IMAGE_MIME_EXT').map(([mime]) => mime);
    expect(rust.length).toBeGreaterThan(0);
    expect([...rust].sort()).toEqual([...ALLOWED_IMAGE_MIMES].sort());
  });

  it('mirrors the HEIC brand list', () => {
    const rust = heicBrands();
    expect(rust.length).toBeGreaterThan(0);
    expect([...rust].sort()).toEqual([...HEIC_BRANDS].sort());
  });
});

describe('both gates name a refused format the same way', () => {
  it('mirrors UNSUPPORTED_IMAGE_FORMATS, id and label', () => {
    const rust = pairTable('UNSUPPORTED_IMAGE_FORMATS');
    const ours = UNSUPPORTED_IMAGE_FORMATS.map((f) => [f.id, f.label]);
    expect([...rust].sort()).toEqual([...ours].sort());
  });

  it('names nothing the engine actually accepts', () => {
    const accepted = new Set(ALLOWED_IMAGE_MIMES.map((m) => m.replace('image/', '').toUpperCase()));
    for (const { id } of UNSUPPORTED_IMAGE_FORMATS) {
      expect(accepted.has(id)).toBe(false);
    }
  });
});

describe('both gates read the same amount of an SVG', () => {
  const declared = /const SVG_SNIFF_LIMIT:\s*usize\s*=\s*(\d+);/.exec(source);
  const limit = Number(declared?.[1]);

  function svgAt(offset: number): Uint8Array {
    const head = '<?xml version="1.0"?>';
    const text = head + ' '.repeat(offset - head.length) + '<svg xmlns="http://x"/>';
    return Uint8Array.from(text, (ch) => ch.charCodeAt(0));
  }

  it('reads a limit out of the Rust source', () => {
    expect(declared, `SVG_SNIFF_LIMIT not found in ${BLOBS_RS}`).not.toBeNull();
    expect(limit).toBeGreaterThan(0);
  });

  it('sees an SVG whose root element ends on the last byte it reads', () => {
    expect(describeUnsupportedImage(svgAt(limit - 4))).toBe('SVG');
  });

  it('misses one that starts a byte later, as the engine does', () => {
    expect(describeUnsupportedImage(svgAt(limit - 3))).toBeNull();
  });
});
