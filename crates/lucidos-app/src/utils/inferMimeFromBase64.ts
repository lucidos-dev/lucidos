export interface PastedImage {
  base64: string;
  mimeType: string;
}

/** Sniff the image MIME type from the first few base64 characters. The
 *  prefixes are stable across encoders because they encode the file's magic
 *  bytes: PNG header `\x89PNG` → "iVBOR", JPEG `\xFF\xD8\xFF` → "/9j/", GIF
 *  `GIF8` → "R0lGOD", WebP `RIFF` → "UklGR". Unknown prefixes fall back to
 *  `image/png` — a safe default that keeps the image loadable even if the
 *  encoder used something exotic. */
export function inferMimeFromBase64(base64: string): string {
  if (base64.startsWith('iVBOR')) return 'image/png';
  if (base64.startsWith('/9j/')) return 'image/jpeg';
  if (base64.startsWith('R0lGOD')) return 'image/gif';
  if (base64.startsWith('UklGR')) return 'image/webp';
  return 'image/png';
}
