import { describe, it, expect } from 'vitest';
import {
  sniffImageBytes,
  sniffImageMime,
  describeUnsupportedImage,
  imageRejectionMessage,
  HEIC_BRANDS,
} from './imageBytes';

function bytes(...parts: (number | string)[]): Uint8Array {
  const out: number[] = [];
  for (const part of parts) {
    if (typeof part === 'number') out.push(part);
    else for (const ch of part) out.push(ch.charCodeAt(0));
  }
  return new Uint8Array(out);
}

/** Pad to `size` so a signature check with a length floor still sees enough. */
function padded(head: Uint8Array, size: number): Uint8Array {
  const out = new Uint8Array(size);
  out.set(head.subarray(0, size));
  return out;
}

const PNG = bytes(0x89, 'PNG', 0x0d, 0x0a, 0x1a, 0x0a, 'IHDR');
const JPEG = bytes(0xff, 0xd8, 0xff, 0xe0, 0x00, 0x10, 'JFIF');
const WEBP = bytes('RIFF', 0x24, 0x00, 0x00, 0x00, 'WEBPVP8 ');
const GIF = bytes('GIF89a', 0x01, 0x00, 0x01, 0x00);
const HEIC = bytes(0x00, 0x00, 0x00, 0x18, 'ftypheic', 0x00, 0x00, 0x00, 0x00);
const TIFF_LE = padded(bytes(0x49, 0x49, 0x2a, 0x00, 0x08), 32);
const TIFF_BE = padded(bytes(0x4d, 0x4d, 0x00, 0x2a, 0x00), 32);
const BMP = padded(bytes('BM', 0x36, 0x00), 32);
const AVIF = bytes(0x00, 0x00, 0x00, 0x1c, 'ftypavif', 0x00, 0x00, 0x00, 0x00);
const PDF = bytes('%PDF-1.7', 0x0a);
const ICO = padded(bytes(0x00, 0x00, 0x01, 0x00, 0x01), 32);
const SVG = bytes('<svg xmlns="http://www.w3.org/2000/svg"/>');

describe('sniffImageMime accepts exactly what the engine stores', () => {
  it.each([
    ['png', PNG, 'image/png'],
    ['jpeg', JPEG, 'image/jpeg'],
    ['webp', WEBP, 'image/webp'],
    ['gif', GIF, 'image/gif'],
    ['heic', HEIC, 'image/heic'],
  ])('reads %s from its magic bytes', (_name, input, mime) => {
    expect(sniffImageMime(input as Uint8Array)).toBe(mime);
  });

  it('reads every HEIC brand the engine reads', () => {
    for (const brand of HEIC_BRANDS) {
      const input = bytes(0x00, 0x00, 0x00, 0x18, 'ftyp', brand, 0x00, 0x00, 0x00, 0x00);
      expect(sniffImageMime(input)).toBe('image/heic');
    }
  });

  it('rejects an ftyp box with a brand outside the HEIC family', () => {
    expect(sniffImageMime(bytes(0x00, 0x00, 0x00, 0x18, 'ftypmp42', 0x00))).toBeNull();
  });

  it('rejects a RIFF container that is not WebP', () => {
    expect(sniffImageMime(bytes('RIFF', 0x24, 0x00, 0x00, 0x00, 'WAVEfmt '))).toBeNull();
  });

  it('rejects a signature cut short', () => {
    expect(sniffImageMime(PNG.subarray(0, 7))).toBeNull();
  });
});

describe('describeUnsupportedImage names what the bytes really are', () => {
  it.each([
    ['little-endian TIFF', TIFF_LE, 'TIFF'],
    ['big-endian TIFF', TIFF_BE, 'TIFF'],
    ['BMP', BMP, 'BMP'],
    ['AVIF', AVIF, 'AVIF'],
    ['PDF', PDF, 'PDF'],
    ['ICO', ICO, 'ICO'],
    ['SVG', SVG, 'SVG'],
  ])('names %s', (_name, input, id) => {
    expect(describeUnsupportedImage(input as Uint8Array)).toBe(id);
  });

  it('names an SVG behind an XML declaration', () => {
    const input = bytes('<?xml version="1.0"?>\n<svg width="1"></svg>');
    expect(describeUnsupportedImage(input)).toBe('SVG');
  });

  it('has no name for bytes that are not a format it knows', () => {
    expect(describeUnsupportedImage(bytes(0x13, 0x37, 0x42, 0x99))).toBeNull();
  });

  it('never names a format the engine accepts', () => {
    for (const accepted of [PNG, JPEG, WEBP, GIF, HEIC]) {
      expect(describeUnsupportedImage(accepted)).toBeNull();
    }
  });
});

describe('sniffImageBytes', () => {
  it('accepts allowlisted bytes and reports the sniffed mime', () => {
    expect(sniffImageBytes(PNG)).toEqual({ kind: 'accepted', mime: 'image/png' });
  });

  it('calls zero bytes empty rather than unsupported', () => {
    expect(sniffImageBytes(new Uint8Array(0))).toEqual({ kind: 'empty' });
  });

  it('carries a label for a format it recognizes', () => {
    expect(sniffImageBytes(TIFF_LE)).toEqual({
      kind: 'unsupported',
      id: 'TIFF',
      label: 'a TIFF image',
    });
  });

  it('carries no label for bytes it cannot place', () => {
    expect(sniffImageBytes(bytes(0x13, 0x37, 0x42, 0x99))).toEqual({
      kind: 'unsupported',
      id: null,
      label: null,
    });
  });

  it('refuses a single stray byte', () => {
    expect(sniffImageBytes(new Uint8Array([0x89]))).toEqual({
      kind: 'unsupported',
      id: null,
      label: null,
    });
  });
});

/** Sniff and phrase in one step, failing loudly if the bytes were accepted. */
function rejection(input: Uint8Array, name: string, declared: string): string {
  const verdict = sniffImageBytes(input);
  if (verdict.kind === 'accepted') throw new Error(`expected a rejection, got ${verdict.mime}`);
  return imageRejectionMessage(verdict, name, declared);
}

describe('imageRejectionMessage', () => {
  it('reports the reported clipboard shape as empty', () => {
    const msg = rejection(new Uint8Array(0), 'image.png', 'image/png');
    expect(msg).toBe('Nothing to attach: "image.png" is empty (0 bytes). Copy or pick the image again.');
  });

  it('names the real format instead of the allowlist', () => {
    const msg = rejection(TIFF_LE, 'shot.tiff', 'image/tiff');
    expect(msg).toBe(`Can't attach "shot.tiff": that's a TIFF image. Save it as PNG or JPEG first.`);
  });

  it('reports the declared mime when the bytes are unplaceable', () => {
    const msg = rejection(bytes(0x13, 0x37), 'image.png', 'image/png');
    expect(msg).toBe(`Can't attach "image.png": those bytes aren't an image Lucidos can read (labelled image/png).`);
  });

  it('drops the parenthetical when nothing declared a type', () => {
    const msg = rejection(bytes(0x13, 0x37), 'mystery', '');
    expect(msg).toBe(`Can't attach "mystery": those bytes aren't an image Lucidos can read.`);
  });

  it('never recites the allowlist', () => {
    for (const shape of [new Uint8Array(0), TIFF_LE, BMP, PDF, SVG, bytes(0x13, 0x37)]) {
      const msg = rejection(shape, 'x', 'image/png');
      expect(msg).not.toContain('allowed:');
      expect(msg).not.toContain('webp');
    }
  });
});
