/**
 * The connection wording table: the mark's readable half, and the sentence every
 * degraded surface states.
 *
 * Two properties carry the whole module. The notice exists for exactly the
 * states the mark RECEDES in and for no others, so no surface can show a dim
 * glyph or a red dot above something that mentions nothing (the stylesheet half
 * of that pairing is asserted in `styles/__tests__/header-mark-geometry.test.ts`:
 * connected is the one state carrying neither an opacity nor an animation). And
 * the sentence exists ONCE, so the menu notice, the header bar, the settings row
 * and the accessible names cannot drift into different claims about one state.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync, readdirSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, join, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';

import { connectionNotice, connectionNoticeSentence, connectionPhrase } from './connectionNotice';
import type { ConnectionStatus } from '../store/types';

const DIMMED: ConnectionStatus[] = ['disconnected', 'connecting'];

/** The mark's readable half. It is the accessible name AND the desktop hover
 *  tooltip, so it has to read as English in all three states and survive the
 *  window before /health has named the workspace. */
describe('connectionPhrase', () => {
  it('names what the mark is connected to, with the preposition each state wants', () => {
    expect(connectionPhrase('connected', 'dev')).toBe('connected to dev');
    expect(connectionPhrase('connecting', 'dev')).toBe('connecting to dev');
    // Not "disconnected TO dev": each state brings its own preposition for
    // exactly this one.
    expect(connectionPhrase('disconnected', 'dev')).toBe('disconnected from dev');
  });

  it('falls back to the bare state before the workspace has a name', () => {
    for (const state of ['connected', 'connecting', 'disconnected']) {
      expect(connectionPhrase(state, null)).toBe(state);
      expect(connectionPhrase(state, '')).toBe(state);
    }
  });

  it('passes an unknown state through rather than inventing a sentence', () => {
    expect(connectionPhrase('reconnecting-soon', 'dev')).toBe('reconnecting-soon');
  });
});

describe('connectionNotice', () => {
  it('speaks for exactly the states the mark dims for', () => {
    for (const state of DIMMED) {
      expect(connectionNotice(state, 'dev'), `${state} leaves the mark receded`).not.toBeNull();
    }
    // The mark is at full strength, so there is nothing to explain.
    expect(connectionNotice('connected', 'dev')).toBeNull();
  });

  it('titles itself with the phrase the toggle already says, sentence-cased', () => {
    // One table, so the tooltip cannot say "disconnected from dev" while the
    // panel says something else. The preposition each state wants is decided in
    // `connectionPhrase` and nowhere twice.
    for (const state of DIMMED) {
      const phrase = connectionPhrase(state, 'dev');
      expect(connectionNotice(state, 'dev')!.title)
        .toBe(phrase.charAt(0).toUpperCase() + phrase.slice(1));
    }
    expect(connectionNotice('disconnected', 'dev')!.title).toBe('Disconnected from dev');
    expect(connectionNotice('connecting', 'dev')!.title).toBe('Connecting to dev');
  });

  it('falls back to the bare state before the workspace has a name', () => {
    // The window before /health answers, which is exactly when the mark is
    // breathing and the notice is most likely to be read.
    expect(connectionNotice('connecting', null)!.title).toBe('Connecting');
    expect(connectionNotice('disconnected', '')!.title).toBe('Disconnected');
  });

  it('promises recovery only where recovery is honest', () => {
    // Neither row below the notice can fix a disconnect: Restart posts to the
    // engine we cannot reach, and Refresh reloads a client that is not what
    // broke. The health poll genuinely does recover on its own, so that is the
    // only thing the line may claim.
    const detail = connectionNotice('disconnected', 'dev')!.detail;
    expect(detail).toContain('Still trying');
    for (const remedy of ['Refresh', 'Restart']) {
      expect(detail, `naming ${remedy} as the fix would be wrong in the ordinary case`)
        .not.toContain(remedy);
    }
  });

  it('claims only this workspace, since the gateway is a different process', () => {
    // `connectionStatus` is driven solely by `/api/v1/health` against this
    // workspace's engine, and the Workspaces row under the notice reaches the
    // GATEWAY instead, so it keeps listing and switching through an engine
    // outage. A blanket "nothing loads or sends" is refuted by the row directly
    // below the sentence making the claim.
    expect(connectionNotice('disconnected', 'dev')!.detail).toContain('in this workspace');
  });
});

describe('connectionNoticeSentence', () => {
  it('joins the two halves for a surface with a name but no room for both', () => {
    // Derived rather than authored, so an accessible name cannot drift from the
    // text rendered beside it.
    const notice = connectionNotice('disconnected', 'dev')!;
    expect(connectionNoticeSentence('disconnected', 'dev'))
      .toBe(`${notice.title}. ${notice.detail}`);
  });

  it('is silent in the state that has nothing to say', () => {
    expect(connectionNoticeSentence('connected', 'dev')).toBeNull();
  });
});

/** The rule the surfaces multiply: four of them now say this, and a fifth will.
 *  A retyped sentence is invisible in review and drifts on the first reword, so
 *  the table is pinned to one file by scanning for its text everywhere else. */
describe('the wording lives in exactly one module', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const srcDir: string = resolve(here, '..');
  const OWNER = resolve(here, 'connectionNotice.ts');

  /** Every `.ts`/`.tsx` under src/, this module and its own test excluded. */
  function sources(dir: string, out: string[] = []): string[] {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        if (entry.name === 'generated' || entry.name === 'node_modules') continue;
        sources(path, out);
      } else if (/\.tsx?$/.test(entry.name) && path !== OWNER && path !== resolve(here, 'connectionNotice.test.ts')) {
        out.push(path);
      }
    }
    return out;
  }

  it('no other source file restates a detail sentence', () => {
    // The literals, not a variable: this is the check that a second copy would
    // fail, and reading them out of the module under test would make it pass on
    // a copy that had drifted.
    const details = [
      'Waiting for the workspace to answer.',
      'Nothing in this workspace loads or sends. Still trying.',
    ];
    for (const file of sources(srcDir)) {
      const text: string = readFileSync(file, 'utf-8');
      for (const detail of details) {
        expect(text, `${file} restates the connection wording; import it instead`)
          .not.toContain(detail);
      }
    }
  });
});
