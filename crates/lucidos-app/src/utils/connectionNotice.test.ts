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
  isEngineUnreachable,
} from './connectionNotice';
import type { ConnectionNoticeStatus } from './connectionNotice';

const DIMMED: ConnectionNoticeStatus[] = ['disconnected', 'connecting', 'stopped', 'starting', 'restarting', 'switching-version'];

describe('connectionPhrase', () => {
  it('names the ENGINE it is reaching, not the workspace that owns it', () => {
    expect(connectionPhrase('connected', 'dev')).toBe('connected to the dev engine');
    expect(connectionPhrase('connecting', 'dev')).toBe('connecting to the dev engine');
    expect(connectionPhrase('disconnected', 'dev')).toBe('cannot reach the dev engine');
  });

  it('says a stopped workspace is stopped, because the gateway said so itself', () => {
    expect(connectionPhrase('stopped', 'dev')).toBe('the dev engine is stopped');
    expect(connectionPhrase('stopped', null)).toBe('the engine is stopped');
  });

  it('says a started workspace is starting, never that it cannot be reached', () => {
    expect(connectionPhrase('starting', 'dev')).toBe('the dev engine is starting');
    expect(connectionNotice('starting', 'dev', 'full')!.detail).not.toContain('Retrying');
  });

  it('claims the reach and never the engine, which is all a failed poll knows', () => {
    const phrase = connectionPhrase('disconnected', 'dev');
    for (const blame of ['disconnected', 'stopped', 'down', 'offline', 'crashed', 'not running']) {
      expect(phrase, `"${blame}" asserts a backend fact the health poll cannot see`)
        .not.toContain(blame);
    }
  });

  it('keeps the noun when the workspace has no name yet', () => {
    for (const nameless of [null, '']) {
      expect(connectionPhrase('connected', nameless)).toBe('connected to the engine');
      expect(connectionPhrase('connecting', nameless)).toBe('connecting to the engine');
      expect(connectionPhrase('disconnected', nameless)).toBe('cannot reach the engine');
    }
  });

  it('names a restart as a restart, never as an unreachable engine', () => {
    expect(connectionPhrase('restarting', 'dev')).toBe('the dev engine is restarting');
    expect(connectionPhrase('switching-version', 'dev')).toBe('the dev engine is starting a new version');
    for (const state of ['restarting', 'switching-version'] as const) {
      expect(connectionNotice(state, 'dev', 'full')!.detail).not.toContain('Retrying');
    }
  });
});

describe('connectionNotice', () => {
  it('speaks for exactly the states the mark dims for', () => {
    for (const state of DIMMED) {
      expect(connectionNotice(state, 'dev', 'full'), `${state} leaves the mark receded`)
        .not.toBeNull();
      expect(hasConnectionNotice(state)).toBe(true);
    }
    expect(connectionNotice('connected', 'dev', 'full')).toBeNull();
    expect(hasConnectionNotice('connected')).toBe(false);
  });

  it('titles itself with the phrase the toggle already says, sentence-cased', () => {
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
    expect(connectionNotice('connecting', null, 'short')!.title).toBe('Connecting to the engine');
    expect(connectionNotice('disconnected', '', 'short')!.title)
      .toBe('Cannot reach the engine');
  });

  it('grows the full form out of the short one, never a second sentence', () => {
    for (const state of DIMMED) {
      const short = connectionNotice(state, 'dev', 'short')!.detail;
      const full = connectionNotice(state, 'dev', 'full')!.detail;
      expect(full.endsWith(short), `${state}'s full detail ends with its short one`).toBe(true);
      expect(full.length).toBeGreaterThan(short.length);
    }
  });

  it('promises recovery only where recovery is honest', () => {
    expect(connectionNotice('disconnected', 'dev', 'short')!.detail)
      .toBe('Retrying every few seconds.');
    for (const remedy of ['Refresh', 'Restart']) {
      expect(connectionNotice('disconnected', 'dev', 'full')!.detail,
        `naming ${remedy} as the fix would be wrong in the ordinary case`)
        .not.toContain(remedy);
    }
  });

  it('names the action that ends a stopped state, and promises no retry', () => {
    const detail = connectionNotice('stopped', 'dev', 'short')!.detail;
    expect(detail).toBe('Start it to continue.');
    expect(detail).not.toContain('Retrying');
  });

  it('names what cannot load and where, since a refutation waits on each', () => {
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

describe('isEngineUnreachable', () => {
  it('is true for a failed poll and a stopped workspace, false otherwise', () => {
    expect(isEngineUnreachable('disconnected')).toBe(true);
    expect(isEngineUnreachable('stopped')).toBe(true);
    expect(isEngineUnreachable('connecting')).toBe(false);
    expect(isEngineUnreachable('starting')).toBe(false);
    expect(isEngineUnreachable('connected')).toBe(false);
  });
});

describe('connectionNoticeSentence', () => {
  it('joins the two halves for a surface with a name but no room for both', () => {
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

describe('the wording lives in exactly one module', () => {
  const here: string = dirname(fileURLToPath(import.meta.url));
  const srcDir: string = resolve(here, '..');
  const OWNER = resolve(here, 'connectionNotice.ts');

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
    const details = [
      'Threads and messages will not load or send in this window yet.',
      'Waiting for an answer.',
      'Threads and messages will not load or send in this window.',
      'Retrying every few seconds.',
      'Start it to continue.',
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
