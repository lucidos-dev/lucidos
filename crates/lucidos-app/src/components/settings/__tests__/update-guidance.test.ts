import { describe, it, expect } from 'vitest';
import { updateGuidance, type UpdateGuidanceInput } from '../updateGuidance';

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
  it('says nothing until the engine has answered', () => {
    expect(updateGuidance(input({ engineAnswered: false, packaged: false }))).toBeNull();
  });

  it('names the source checkout, which downloads nothing', () => {
    expect(updateGuidance(input({ packaged: false }))).toBe('source-checkout');
  });

  it('says nothing where this session can install the offer', () => {
    expect(updateGuidance(input({ hasOffer: true, sessionCanInstall: true })))
      .toBeNull();
  });

  it('never tells the desktop client to go and find the desktop client', () => {
    const client = input({ sessionCanInstall: true, canCheckHere: false });
    expect(updateGuidance(client)).toBeNull();
    expect(updateGuidance({ ...client, install: null })).toBeNull();
  });

  it('says nothing for a headless install, which has its command', () => {
    expect(updateGuidance(input({ hasOffer: true, install: 'installer-rerun' })))
      .toBeNull();
  });

  it('tells a browser session where the install happens', () => {
    expect(updateGuidance(input({ hasOffer: true }))).toBe('install-in-the-app');
  });

  it('answers a session with no offer and no check to run', () => {
    expect(updateGuidance(input({ canCheckHere: false, install: null })))
      .toBe('install-in-the-app');
  });

  it('says nothing while a check is still worth pressing', () => {
    expect(updateGuidance(input({ canCheckHere: true }))).toBeNull();
    expect(updateGuidance(input({ canCheckHere: true, install: null }))).toBeNull();
  });

  it('answers whenever the page has no control of its own', () => {
    for (const hasOffer of [true, false]) {
      for (const install of ['desktop-app', null] as const) {
        const shape = input({ hasOffer, install, canCheckHere: false });
        expect(updateGuidance(shape), `${hasOffer}/${install}`).not.toBeNull();
      }
    }
  });
});
