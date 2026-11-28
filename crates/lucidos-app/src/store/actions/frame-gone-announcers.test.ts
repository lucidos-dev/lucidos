import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';


const here: string = dirname(fileURLToPath(import.meta.url));
const appFrame = readFileSync(resolve(here, '../../components/apps/AppFrame.tsx'), 'utf-8');
const preview = readFileSync(resolve(here, '../../components/files/FilePreviewInline.tsx'), 'utf-8');

describe('a frame says when its document goes away', () => {
  it('announces each app frame element on its unmount or replacement', () => {
    const effect = appFrame.match(/useLayoutEffect\(\(\) => \{\s*const frameWindow[\s\S]*?\}, \[generation\]\);/)?.[0] ?? '';
    expect(effect).toMatch(/const frameWindow = iframeRef\.current\?\.contentWindow \?\? null;/);
    expect(effect).toMatch(/return \(\) => announceFrameGone\(frameWindow\);/);
  });

  it('announces each stamped preview document from the effect keyed on its nonce', () => {
    const effect = preview.match(/useLayoutEffect\(\(\) => \{\s*const documentWindow[\s\S]*?\}, \[nonce, path, declaresOwnBase\]\);/)?.[0] ?? '';
    expect(effect).toMatch(/const documentWindow = frameRef\.current\?\.contentWindow \?\? null;/);
    expect(effect).toMatch(/return \(\) => \{[\s\S]*announceFrameGone\(documentWindow\);/);
  });
});
