import { describe, it, expect } from 'vitest';
import { handlerBody, stripComments } from '../__tests__/sourceScan';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const SOURCE = resolve(here, 'app-frame-messages.ts');

describe('app frame toast bridge wiring', () => {
  const src = stripComments(readFileSync(SOURCE, 'utf8'));
  const body = handlerBody(src, 'function onAppFrameMessage(');

  it('admits the dismiss message type past the allow-list', () => {
    expect(body, 'a type missing from the allow-list never reaches any branch')
      .toContain(`data.type !== 'lucidos:ui:dismissToast'`);
  });

  it('routes the toast bridge BEFORE the message guard that would swallow a dismiss', () => {
    const bridgeAt = body.indexOf('handleAppToastMessage(');
    const guardAt = body.indexOf(`typeof payload.message !== 'string'`);
    expect(bridgeAt, 'the toast bridge must be wired into the handler').toBeGreaterThan(-1);
    expect(guardAt).toBeGreaterThan(-1);
    expect(guardAt, 'a dismiss carries no message, so the guard must come second').toBeGreaterThan(bridgeAt);
  });

  it('keeps the frame-authenticity check ahead of both', () => {
    const frameAt = body.indexOf('isKnownAppFrame(source)');
    expect(frameAt).toBeGreaterThan(-1);
    expect(body.indexOf('handleAppToastMessage(')).toBeGreaterThan(frameAt);
  });
});
