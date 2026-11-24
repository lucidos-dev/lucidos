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
    expect(cameraIsAvailableIn({ mediaDevices: { getUserMedia }, secureContext: false })).toBe(
      false,
    );
    expect(cameraIsAvailableIn({ mediaDevices: undefined, secureContext: true })).toBe(false);
    expect(cameraIsAvailableIn({ mediaDevices: {}, secureContext: true })).toBe(false);
  });

  it('offers Scan only on a mobile client that can actually open one', () => {
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
    expect(() => stopTracks(null)).not.toThrow();
  });

  it('stops on unmount, and on a decode before the parent hears about it', () => {
    expect(scannerSrc).toMatch(/return \(\) => \{\s*done = true;\s*stop\(\);\s*\};/);
    expect(scannerSrc).toMatch(/stop\(\);\s*onCode\(found\);/);
  });

  it('stops a stream that arrives after the screen is gone', () => {
    expect(scannerSrc).toMatch(/if \(done\) \{\s*stopTracks\(stream\);/);
  });
});

describe('the decoder stays off the first paint', () => {
  it('reaches the scanner through lazyComponent', () => {
    expect(gateSrc).toMatch(/lazyComponent[\s\S]{0,120}?import\('\.\/PairingScanner'\)/);
    expect(scannerSrc).toContain("lazyModule(() => import('jsqr'))");
    expect(scannerSrc).not.toMatch(/^import (?!type ).* from 'jsqr';$/m);
    expect(gateSrc).not.toMatch(/^import .* from '\.\/PairingScanner';$/m);
  });

  it('gives jsqr its own chunk, so the vendor catch-all cannot eagerly ship it', () => {
    expect(viteConfig).toMatch(/if \(id\.includes\('jsqr'\)\) return 'jsqr';/);
  });
});
