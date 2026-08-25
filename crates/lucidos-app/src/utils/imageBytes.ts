/**
 * Magic-byte image sniffing on the client, mirroring the engine.
 *
 * The blob store decides what it accepts by reading the bytes
 * (`sniff_image_mime` in `crates/lucidos-engine/src/core/blobs.rs`), never the
 * MIME a client declares. A composer that gates on the declared type builds a
 * chip for bytes the upload then refuses. This module asks the server's own
 * question on the client, before anything is drawn.
 *
 * The three tables below are pinned to the Rust source by
 * `imageBytes.mirror.test.ts`.
 */

/** Mimes the engine's blob store stores. Mirrors `ALLOWED_IMAGE_MIME_EXT`. */
export const ALLOWED_IMAGE_MIMES = [
  'image/png',
  'image/jpeg',
  'image/webp',
  'image/gif',
  'image/heic',
] as const;

export type AllowedImageMime = (typeof ALLOWED_IMAGE_MIMES)[number];

/** `ftyp` brands the engine reads as HEIC. */
export const HEIC_BRANDS = ['heic', 'heix', 'hevc', 'hevx', 'mif1', 'msf1'] as const;

/**
 * Formats recognized only to name them in a rejection. Nothing here is
 * accepted; the label is what the user reads instead of an allowlist.
 */
export const UNSUPPORTED_IMAGE_FORMATS = [
  { id: 'TIFF', label: 'a TIFF image' },
  { id: 'BMP', label: 'a BMP image' },
  { id: 'AVIF', label: 'an AVIF image' },
  { id: 'SVG', label: 'an SVG vector image' },
  { id: 'PDF', label: 'a PDF document' },
  { id: 'ICO', label: 'an icon file' },
] as const;

export type UnsupportedImageId = (typeof UNSUPPORTED_IMAGE_FORMATS)[number]['id'];

/** A verdict that refuses the bytes, and therefore owes the user a reason. */
export type RejectedImage =
  | { kind: 'empty' }
  | { kind: 'unsupported'; id: UnsupportedImageId | null; label: string | null };

export type ImageVerdict = { kind: 'accepted'; mime: AllowedImageMime } | RejectedImage;

function matches(bytes: Uint8Array, signature: readonly number[], offset = 0): boolean {
  if (bytes.length < offset + signature.length) return false;
  for (let i = 0; i < signature.length; i++) {
    if (bytes[offset + i] !== signature[i]) return false;
  }
  return true;
}

function asciiCodes(text: string): number[] {
  return Array.from(text, (ch) => ch.charCodeAt(0));
}

function asciiAt(bytes: Uint8Array, offset: number, length: number): string {
  if (bytes.length < offset + length) return '';
  let out = '';
  for (let i = 0; i < length; i++) out += String.fromCharCode(bytes[offset + i]);
  return out;
}

/**
 * The engine's `sniff_image_mime`, byte for byte. Returns null for anything
 * outside the allowlist.
 */
export function sniffImageMime(bytes: Uint8Array): AllowedImageMime | null {
  if (matches(bytes, [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a])) return 'image/png';
  if (matches(bytes, [0xff, 0xd8, 0xff])) return 'image/jpeg';
  if (bytes.length >= 12 && matches(bytes, asciiCodes('RIFF')) && matches(bytes, asciiCodes('WEBP'), 8)) {
    return 'image/webp';
  }
  if (matches(bytes, asciiCodes('GIF87a')) || matches(bytes, asciiCodes('GIF89a'))) return 'image/gif';
  if (bytes.length >= 12 && matches(bytes, asciiCodes('ftyp'), 4)) {
    const brand = asciiAt(bytes, 8, 4);
    if ((HEIC_BRANDS as readonly string[]).includes(brand)) return 'image/heic';
  }
  return null;
}

/** How much of a text file to read when looking for an SVG root element. */
const SVG_SNIFF_LIMIT = 512;

function looksLikeSvg(bytes: Uint8Array): boolean {
  const head = asciiAt(bytes, 0, Math.min(bytes.length, SVG_SNIFF_LIMIT)).toLowerCase();
  const start = head.trimStart();
  if (start.startsWith('<svg')) return true;
  return start.startsWith('<?xml') && head.includes('<svg');
}

/**
 * Name a format the engine refuses, so the rejection can say what the bytes
 * really are. Diagnosis only: every id here is still a refusal.
 */
export function describeUnsupportedImage(bytes: Uint8Array): UnsupportedImageId | null {
  if (matches(bytes, [0x49, 0x49, 0x2a, 0x00]) || matches(bytes, [0x4d, 0x4d, 0x00, 0x2a])) return 'TIFF';
  if (matches(bytes, asciiCodes('BM'))) return 'BMP';
  if (bytes.length >= 12 && matches(bytes, asciiCodes('ftyp'), 4)) {
    const brand = asciiAt(bytes, 8, 4);
    if (brand === 'avif' || brand === 'avis') return 'AVIF';
  }
  if (matches(bytes, asciiCodes('%PDF-'))) return 'PDF';
  if (matches(bytes, [0x00, 0x00, 0x01, 0x00])) return 'ICO';
  if (looksLikeSvg(bytes)) return 'SVG';
  return null;
}

function labelFor(id: UnsupportedImageId): string {
  const found = UNSUPPORTED_IMAGE_FORMATS.find((f) => f.id === id);
  return found ? found.label : id;
}

/** Decide an attachment from its bytes alone, the way the engine does. */
export function sniffImageBytes(bytes: Uint8Array): ImageVerdict {
  if (bytes.length === 0) return { kind: 'empty' };
  const mime = sniffImageMime(bytes);
  if (mime) return { kind: 'accepted', mime };
  const id = describeUnsupportedImage(bytes);
  return { kind: 'unsupported', id, label: id ? labelFor(id) : null };
}

/**
 * What the user reads at the moment of the paste. Names this attachment and
 * this format; never recites the allowlist.
 */
export function imageRejectionMessage(
  verdict: RejectedImage,
  name: string,
  declaredMime: string,
): string {
  const named = name ? `"${name}"` : 'that image';
  if (verdict.kind === 'empty') {
    return `Nothing to attach: ${named} is empty (0 bytes). Copy or pick the image again.`;
  }
  if (verdict.label) {
    return `Can't attach ${named}: that's ${verdict.label}. Save it as PNG or JPEG first.`;
  }
  const labelled = declaredMime ? ` (labelled ${declaredMime})` : '';
  return `Can't attach ${named}: those bytes aren't an image Lucidos can read${labelled}.`;
}
