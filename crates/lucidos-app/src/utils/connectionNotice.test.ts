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

import {
  connectionNotice,
  connectionNoticeSentence,
  connectionPhrase,
  hasConnectionNotice,
} from './connectionNotice';
import type { ConnectionStatus } from '../store/types';

const DIMMED: ConnectionStatus[] = ['disconnected', 'connecting'];

/** The mark's readable half. It is the accessible name AND the desktop hover
 *  tooltip, so it has to read as English in all three states and survive the
 *  window before /health has named the workspace. */
describe('connectionPhrase', () => {
  it('names the ENGINE it is reaching, not the workspace that owns it', () => {
    // The workspace has not gone anywhere: the gateway keeps listing and
    // switching workspaces throughout an outage. What this client lost is the
    // route to the process serving this one. Saying so is the difference
    // between an alarming claim and a true one.
    expect(connectionPhrase('connected', 'dev')).toBe('connected to the dev engine');
    expect(connectionPhrase('connecting', 'dev')).toBe('connecting to the dev engine');
    expect(connectionPhrase('disconnected', 'dev')).toBe('cannot reach the dev engine');
  });

  it('claims the reach and never the engine, which is all a failed poll knows', () => {
    // `connectionStatus` flips on consecutive failures of ONE request. That
    // failure cannot tell an engine that stopped from a client that cannot
    // reach a healthy one. A packaged desktop window hit the second case while
    // the gateway served 200s, and a phone worked against the same stack.
    const phrase = connectionPhrase('disconnected', 'dev');
    for (const blame of ['disconnected', 'stopped', 'down', 'offline', 'crashed', 'not running']) {
      expect(phrase, `"${blame}" asserts a backend fact the health poll cannot see`)
        .not.toContain(blame);
    }
  });

  it('keeps the noun when the workspace has no name yet', () => {
    // Before /health first answers there is nothing to name. A title of one
    // bare state word would leave the sentence under it with nothing to be
    // about.
    for (const nameless of [null, '']) {
      expect(connectionPhrase('connected', nameless)).toBe('connected to the engine');
      expect(connectionPhrase('connecting', nameless)).toBe('connecting to the engine');
      expect(connectionPhrase('disconnected', nameless)).toBe('cannot reach the engine');
    }
  });

  it('passes an unknown state through rather than inventing a sentence', () => {
    expect(connectionPhrase('reconnecting-soon', 'dev')).toBe('reconnecting-soon');
  });
});

describe('connectionNotice', () => {
  it('speaks for exactly the states the mark dims for', () => {
    for (const state of DIMMED) {
      expect(connectionNotice(state, 'dev', 'full'), `${state} leaves the mark receded`)
        .not.toBeNull();
      // The bar decides whether to exist at all from this, so it has to answer
      // for the same set the words are written for.
      expect(hasConnectionNotice(state)).toBe(true);
    }
    // The mark is at full strength, so there is nothing to explain.
    expect(connectionNotice('connected', 'dev', 'full')).toBeNull();
    expect(hasConnectionNotice('connected')).toBe(false);
  });

  it('titles itself with the phrase the toggle already says, sentence-cased', () => {
    // One table, so the tooltip cannot name one thing while the panel names
    // another. The preposition each state wants is decided in
    // `connectionPhrase` and nowhere twice.
    for (const state of DIMMED) {
      const phrase = connectionPhrase(state, 'dev');
      expect(connectionNotice(state, 'dev', 'short')!.title)
        .toBe(phrase.charAt(0).toUpperCase() + phrase.slice(1));
    }
    expect(connectionNotice('disconnected', 'dev', 'short')!.title)
      .toBe('Cannot reach the dev engine');
    expect(connectionNotice('connecting', 'dev', 'short')!.title)
      .toBe('Connecting to the dev engine');
  });

  it('still names the engine before the workspace has a name', () => {
    // The window before /health answers, which is exactly when the mark is
    // breathing and the notice is most likely to be read.
    expect(connectionNotice('connecting', null, 'short')!.title).toBe('Connecting to the engine');
    expect(connectionNotice('disconnected', '', 'short')!.title)
      .toBe('Cannot reach the engine');
  });

  it('grows the full form out of the short one, never a second sentence', () => {
    // The two lengths are one table read twice: full is the consequence with
    // the short form after it. Two authored strings would drift on the first
    // reword, which is why the clauses are stored apart.
    for (const state of DIMMED) {
      const short = connectionNotice(state, 'dev', 'short')!.detail;
      const full = connectionNotice(state, 'dev', 'full')!.detail;
      expect(full.endsWith(short), `${state}'s full detail ends with its short one`).toBe(true);
      expect(full.length).toBeGreaterThan(short.length);
    }
  });

  it('promises recovery only where recovery is honest', () => {
    // Neither row below the notice can fix a disconnect: Restart posts to the
    // engine we cannot reach, and Refresh reloads a client that is not what
    // broke. The health poll genuinely does recover on its own, so that is the
    // only thing the line may claim.
    // The short form is that claim alone, at the interval the poll runs at.
    expect(connectionNotice('disconnected', 'dev', 'short')!.detail)
      .toBe('Retrying every few seconds.');
    for (const remedy of ['Refresh', 'Restart']) {
      expect(connectionNotice('disconnected', 'dev', 'full')!.detail,
        `naming ${remedy} as the fix would be wrong in the ordinary case`)
        .not.toContain(remedy);
    }
  });

  it('names what cannot load and where, since a refutation waits on each', () => {
    // The Workspaces row reaches the GATEWAY, so it keeps listing and switching
    // through an engine outage, from inside this very window. A location scope
    // therefore cannot carry the claim on its own. And a phone on the same
    // gateway can be loading and sending fine, which is what happened. So the
    // copy owes the location scope too.
    // The short form makes neither claim, which is what lets the menu state it
    // directly above a switcher that still works.
    for (const state of DIMMED) {
      const full = connectionNotice(state, 'dev', 'full')!.detail;
      expect(full, `${state} must name the engine's own content, not the window's whole UI`)
        .toContain('Threads and messages');
      expect(full, `${state} must scope to this client`).toContain('in this window');
      expect(full).not.toContain('in this workspace');
    }
    expect(connectionNotice('disconnected', 'dev', 'short')!.detail)
      .not.toContain('load or send');
  });
});

describe('connectionNoticeSentence', () => {
  it('joins the two halves for a surface with a name but no room for both', () => {
    // Derived rather than authored, so an accessible name cannot drift from the
    // text rendered beside it.
    const notice = connectionNotice('disconnected', 'dev', 'short')!;
    expect(connectionNoticeSentence('disconnected', 'dev', 'short'))
      .toBe(`${notice.title}. ${notice.detail}`);
  });

  it('carries whichever length it was asked for', () => {
    expect(connectionNoticeSentence('disconnected', 'dev', 'full'))
      .toContain(connectionNotice('disconnected', 'dev', 'full')!.detail);
  });

  it('is silent in the state that has nothing to say', () => {
    expect(connectionNoticeSentence('connected', 'dev', 'full')).toBeNull();
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
    // a copy that had drifted. Every clause of every length, since a surface
    // wanting a shorter line is exactly the thing that retypes a trimmed one.
    const details = [
      'Threads and messages will not load or send in this window yet.',
      'Waiting for an answer.',
      'Threads and messages will not load or send in this window.',
      'Retrying every few seconds.',
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
