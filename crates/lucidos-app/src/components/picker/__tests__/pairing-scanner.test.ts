/**
 * The camera scanner on the pairing screen.
 *
 * The component itself uses hooks and a real `MediaStream`, neither of which
 * this suite has, so what it does is pinned by source scan. The two things that
 * can be decided are decided: which clients see the button, and that stopping a
 * stream stops all of it.
 *
 * The decoder is the reason for the chunk assertions. `jsqr` is 130 kB, and the
 * pairing screen is the first paint of a cold PWA launch.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { stopTracks } from '../PairingScanner';
import { cameraIsAvailableIn } from '../../../utils/platform';

const here = dirname(fileURLToPath(import.meta.url));
const gateSrc: string = readFileSync(resolve(here, '../PairingGate.tsx'), 'utf8');
const scannerSrc: string = readFileSync(resolve(here, '../PairingScanner.tsx'), 'utf8');
const viteConfig: string = readFileSync(resolve(here, '../../../../vite.config.ts'), 'utf8');

describe('where a camera can be opened', () => {
  const getUserMedia = () => Promise.resolve({} as MediaStream);

  it('needs a secure context AND a mediaDevices', () => {
    expect(cameraIsAvailableIn({ mediaDevices: { getUserMedia }, secureContext: true })).toBe(true);
  });

  it('refuses a plain-http LAN origin, which is the case that bites', () => {
    // WebKit still exposes `mediaDevices` there and then refuses the call, so
    // the secure-context half cannot be inferred from the other one.
    expect(cameraIsAvailableIn({ mediaDevices: { getUserMedia }, secureContext: false })).toBe(
      false,
    );
    expect(cameraIsAvailableIn({ mediaDevices: undefined, secureContext: true })).toBe(false);
    expect(cameraIsAvailableIn({ mediaDevices: {}, secureContext: true })).toBe(false);
  });

  it('offers Scan only on a mobile client that can actually open one', () => {
    // A laptop has a camera and no QR in front of it, and the machine running
    // Lucidos pairs its own window.
    expect(gateSrc).toContain('thisDeviceIsMobile() && cameraIsAvailable()');
  });
});

describe('the stream is always stopped', () => {
  it('stops every track, not just the first', () => {
    const stopped: string[] = [];
    const track = (id: string) => ({ stop: () => stopped.push(id) });
    const stream = { getTracks: () => [track('video'), track('audio')] } as unknown as MediaStream;
    stopTracks(stream);
    expect(stopped).toEqual(['video', 'audio']);
  });

  it('tolerates having no stream at all', () => {
    // The permission prompt can be refused before one ever exists.
    expect(() => stopTracks(null)).not.toThrow();
  });

  it('stops on unmount, and on a decode before the parent hears about it', () => {
    // A live camera keeps the phone's recording indicator lit, so each of these
    // is a leak the user can see.
    expect(scannerSrc).toMatch(/return \(\) => \{\s*doneRef\.current = true;\s*stop\(\);\s*\};/);
    expect(scannerSrc).toMatch(/stop\(\);\s*onCode\(found\);/);
  });

  it('stops a stream that arrives after the screen is gone', () => {
    // getUserMedia resolves whenever the user answers the prompt, which can be
    // long after they moved on. Nothing else would ever stop that stream.
    expect(scannerSrc).toMatch(/if \(doneRef\.current\) \{\s*stopTracks\(stream\);/);
  });
});

describe('the decoder stays off the first paint', () => {
  it('reaches the scanner through lazyComponent', () => {
    expect(gateSrc).toMatch(/lazyComponent[\s\S]{0,120}?import\('\.\/PairingScanner'\)/);
    // A static import would put the decoder in the entry whatever the chunk
    // config said.
    expect(gateSrc).not.toMatch(/^import .* from '\.\/PairingScanner';$/m);
  });

  it('gives jsqr its own chunk, so the vendor catch-all cannot eagerly ship it', () => {
    // `vendor` is loaded by the entry, so an unnamed lazy dependency lands on
    // first paint anyway. This is the same reason highlight.js is named.
    expect(viteConfig).toMatch(/if \(id\.includes\('jsqr'\)\) return 'jsqr';/);
  });
});
