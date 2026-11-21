/**
 * Settings, System, Maintenance: the sentence that answers what its controls
 * cannot.
 *
 * `updateRoute`'s `guide` lands readers on that page (ADR 0142), so it owes an
 * answer to every install shape that arrives. These pin both halves: that each
 * shape gets one, and that the guidance never repeats a control beside it.
 */
import { describe, it, expect } from 'vitest';
import { updateGuidance, type UpdateGuidanceInput } from '../updateGuidance';

/** A browser session on a packaged bundle, with nothing on offer yet. */
function input(over: Partial<UpdateGuidanceInput> = {}): UpdateGuidanceInput {
  return {
    engineAnswered: true,
    packaged: true,
    hasOffer: false,
    sessionCanInstall: false,
    canCheckHere: true,
    install: 'desktop-app',
    ...over,
  };
}

describe('updateGuidance', () => {
  // `packaged` reads false until /health answers, and a user whose engine is
  // down comes to this very page. Asserting a source checkout there is a lie
  // told in the one place it does most damage.
  it('says nothing until the engine has answered', () => {
    expect(updateGuidance(input({ engineAnswered: false, packaged: false }))).toBeNull();
  });

  it('names the source checkout, which downloads nothing', () => {
    expect(updateGuidance(input({ packaged: false }))).toBe('source-checkout');
  });

  // The button beside it already says "Update & Restart".
  it('says nothing where this session can install the offer', () => {
    expect(updateGuidance(input({ hasOffer: true, sessionCanInstall: true })))
      .toBeNull();
  });

  // It IS the app, so telling it where the app is would be absurd. Reachable
  // with a packaged engine behind a gateway started from a checkout.
  it('never tells the desktop client to go and find the desktop client', () => {
    const client = input({ sessionCanInstall: true, canCheckHere: false });
    expect(updateGuidance(client)).toBeNull();
    expect(updateGuidance({ ...client, install: null })).toBeNull();
  });

  // The gateway composes the installer command, and it is rendered right there.
  it('says nothing for a headless install, which has its command', () => {
    expect(updateGuidance(input({ hasOffer: true, install: 'installer-rerun' })))
      .toBeNull();
  });

  // The reported loop: the offer is real, this session cannot take it, and the
  // page said only the version. Check re-found the release and sent them back.
  it('tells a browser session where the install happens', () => {
    expect(updateGuidance(input({ hasOffer: true }))).toBe('install-in-the-app');
  });

  // No gateway answer and no client updater, on a packaged install. There is no
  // button at all, so the page would otherwise be blank on the one question.
  it('answers a session with no offer and no check to run', () => {
    expect(updateGuidance(input({ canCheckHere: false, install: null })))
      .toBe('install-in-the-app');
  });

  // A check IS the answer while there is no offer yet, whatever the session is.
  it('says nothing while a check is still worth pressing', () => {
    expect(updateGuidance(input({ canCheckHere: true }))).toBeNull();
    expect(updateGuidance(input({ canCheckHere: true, install: null }))).toBeNull();
  });

  // The rule, as the property: this page never leaves the question unanswered.
  // A shape with no control and no sentence is the dead end `guide` created.
  it('answers whenever the page has no control of its own', () => {
    for (const hasOffer of [true, false]) {
      for (const install of ['desktop-app', null] as const) {
        const shape = input({ hasOffer, install, canCheckHere: false });
        expect(updateGuidance(shape), `${hasOffer}/${install}`).not.toBeNull();
      }
    }
  });
});
