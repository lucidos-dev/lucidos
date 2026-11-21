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

/**
 * The app-facing toast bridge reaches the handler at all.
 *
 * `lucidos.ui.dismissToast(key)` posts a payload carrying a key and NO message.
 * Two filters in `onAppFrameMessage` would each swallow it silently: the
 * message-type allow-list at the top, and the "confirm and prompt carry a
 * message" early return further down. Neither leaves an error anywhere. From
 * the app's side the dismiss does nothing, and its spinner spins forever.
 *
 * What the bridge DOES once reached is behaviour, tested as behaviour in
 * `components/shared/__tests__/toast-app-bridge.test.tsx`. Only the two ordering
 * facts that live in this file are pinned here, for the same reason as the
 * guards in `startup.test.ts`: the routing needs a real frame to reach.
 */
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
    // An unattributed frame gets no host chrome, whatever it asked for: a nested
    // embed must not be able to clear a toast the real app is showing.
    const frameAt = body.indexOf('isKnownAppFrame(source)');
    expect(frameAt).toBeGreaterThan(-1);
    expect(body.indexOf('handleAppToastMessage(')).toBeGreaterThan(frameAt);
  });
});
