import { describe, it, expect, vi } from 'vitest';
import { resolveFileTarget } from './fileTarget';

// fileTarget imports the real `normalizeDataPath` from ./artifacts (the point of
// this suite is that the resolver runs the genuine locator rules). Its module
// siblings reach for the API client and the pane/nav actions at import time, so
// stub those the way artifacts.test.ts does.
vi.mock('../../api/client', () => ({ listArtifacts: vi.fn(), uploadFile: vi.fn() }));
vi.mock('./pane', () => ({ revealContentPane: vi.fn() }));
vi.mock('./navigation', () => ({ pushNavState: vi.fn() }));

const REPO_ID = '3f9c1b2e-0d44-4a71-9f6d-2e5b8c7a1d03';

describe('resolveFileTarget: the locator', () => {
  it('treats a prefix-less path as an artifact', () => {
    expect(resolveFileTarget('notes.md').path).toBe('artifacts/notes.md');
  });

  it.each(['artifacts/notes.md', 'knowhow/x/guide.md', 'apps/a/index.html', 'system-knowhow/js-sdk.md'])(
    'leaves the known data prefix %s alone',
    (path) => {
      expect(resolveFileTarget(path).path).toBe(path);
    },
  );

  it('passes a repo-encoded path through intact and parses it', () => {
    const encoded = `repo:${REPO_ID}:file:src/main.rs`;
    const target = resolveFileTarget(encoded);
    expect(target.path).toBe(encoded);
    expect(target.repo).toEqual({ repoId: REPO_ID, mode: 'file', path: 'src/main.rs' });
  });

  it('carries the change id of a diff-encoded path', () => {
    const target = resolveFileTarget(`repo:${REPO_ID}:diff#change-7:src/main.rs`);
    expect(target.repo).toEqual({
      repoId: REPO_ID, mode: 'diff', changeId: 'change-7', path: 'src/main.rs',
    });
  });

  // A structurally incomplete encoding is NOT a repo path. It degrades to the
  // artifact rule rather than opening a preview that can only 404.
  it.each(['repo::file:x', 'repo:r1:file:', 'repo:r1:bogus:x'])(
    'treats the malformed encoding %s as a data path',
    (encoded) => {
      const target = resolveFileTarget(encoded);
      expect(target.repo).toBeNull();
      expect(target.path).toBe(`artifacts/${encoded}`);
    },
  );
});

describe('resolveFileTarget: the line range', () => {
  it('has no range when no line is given', () => {
    expect(resolveFileTarget('artifacts/notes.md').range).toBeNull();
  });

  it('resolves a single line', () => {
    expect(resolveFileTarget('artifacts/notes.md', 510).range).toEqual({ start: 510, end: 510 });
  });

  it('resolves an inclusive range', () => {
    expect(resolveFileTarget('artifacts/notes.md', 510, 520).range).toEqual({ start: 510, end: 520 });
  });

  it('swaps an inverted range rather than dropping it', () => {
    expect(resolveFileTarget('artifacts/notes.md', 520, 510).range).toEqual({ start: 510, end: 520 });
  });

  // A citation's line number is the part that goes stale. It must never cost the
  // reader the file itself: the path still resolves, only the range is dropped.
  it.each([
    ['zero', 0],
    ['negative', -3],
    ['fractional', 1.5],
    ['a string', 'abc'],
    ['null', null],
  ])('drops an unusable line (%s) but still resolves the file', (_label, line) => {
    const target = resolveFileTarget('artifacts/notes.md', line);
    expect(target.path).toBe('artifacts/notes.md');
    expect(target.range).toBeNull();
  });

  // The line count isn't known until the content loads, so a past-the-end line is
  // accepted here and highlights nothing; LineNumberedCode drops the selection
  // once it can see the file is too short.
  it('accepts a line past the end of the file', () => {
    expect(resolveFileTarget('artifacts/notes.md', 9_000_000).range)
      .toEqual({ start: 9_000_000, end: 9_000_000 });
  });

  // A target that can never show numbered lines must not end up with an
  // invisible selection: `currentChatContext` would attach it to the next
  // message as a range naming no code.
  it.each([
    ['a PDF', 'artifacts/report.pdf'],
    ['an image', 'artifacts/chart.png'],
    ['a video', 'artifacts/clip.mp4'],
    ['audio', 'artifacts/note.m4a'],
  ])('drops the line for %s, which has no source view', (_label, path) => {
    expect(resolveFileTarget(path, 5).range).toBeNull();
  });

  it('drops the line for a repo diff, whose hunks carry their own numbering', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:diff#change-7:src/main.rs`, 5).range).toBeNull();
  });

  it('keeps the line for a repo file', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:file:src/main.rs`, 5).range).toEqual({ start: 5, end: 5 });
  });

  it('keeps the line for an extensionless file, which is textual by default', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:file:Makefile`, 5).range).toEqual({ start: 5, end: 5 });
  });

  // The media check reads the extension off the repo-relative path, not off the
  // encoded locator (whose last segment would otherwise decide it).
  it('reads the extension from inside a repo locator', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:file:docs/report.pdf`, 5).range).toBeNull();
  });

  it('carries the ref of a file locator that names one', () => {
    const target = resolveFileTarget(`repo:${REPO_ID}:file#origin/main:src/main.rs`, 5);
    expect(target.repo).toEqual({
      repoId: REPO_ID, mode: 'file', ref: 'origin/main', path: 'src/main.rs',
    });
    expect(target.range).toEqual({ start: 5, end: 5 });
  });
});

/** The navigate router and the app-facing preview modal share this resolver so
 *  the modal cannot reach a file, or honour a line, that `navigate('file', …)`
 *  would not. The view input is the ONE thing they differ by, and this is what
 *  pins how far that difference reaches: exactly one line rule, and never the
 *  resolved path. */
describe('resolveFileTarget: the two views', () => {
  const LOCATORS = [
    'artifacts/notes.md',
    'notes.md',
    'artifacts/report.pdf',
    `repo:${REPO_ID}:file:src/main.rs`,
    `repo:${REPO_ID}:file#v1.2.0:src/main.rs`,
    `repo:${REPO_ID}:diff#change-7:src/main.rs`,
    'repo::file:x',
  ];

  it('resolves the same path and the same locator for either view', () => {
    for (const locator of LOCATORS) {
      const asEncoded = resolveFileTarget(locator, 5, 9, 'as-encoded');
      const asFile = resolveFileTarget(locator, 5, 9, 'file');
      expect(asFile.path).toBe(asEncoded.path);
      expect(asFile.repo).toEqual(asEncoded.repo);
    }
  });

  it('differs only on a diff locator, and only in the line range', () => {
    for (const locator of LOCATORS) {
      const asEncoded = resolveFileTarget(locator, 5, 9, 'as-encoded');
      const asFile = resolveFileTarget(locator, 5, 9, 'file');
      if (asEncoded.repo?.mode === 'diff') continue;
      expect(asFile.range).toEqual(asEncoded.range);
    }
  });

  // A caller rendering the FILE is showing the file's own line numbers, so the
  // citation is honourable there even though the locator names a diff. The
  // locator itself is untouched, which is how the change id survives to
  // `RepoFileContent`.
  it('honours a citation into a diff locator for a caller that renders the file', () => {
    const locator = `repo:${REPO_ID}:diff#change-7:src/main.rs`;
    const target = resolveFileTarget(locator, 510, 520, 'file');
    expect(target.path).toBe(locator);
    expect(target.repo).toEqual({
      repoId: REPO_ID, mode: 'diff', changeId: 'change-7', path: 'src/main.rs',
    });
    expect(target.range).toEqual({ start: 510, end: 520 });
  });

  // The view moves the DIFF rule and nothing else: a PDF has no source view in
  // either, so its line is still dropped.
  it('still drops a line the file itself cannot show', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:diff#change-7:docs/report.pdf`, 5, undefined, 'file').range)
      .toBeNull();
  });

  it('defaults to the as-encoded view', () => {
    expect(resolveFileTarget(`repo:${REPO_ID}:diff#change-7:src/main.rs`, 5).range).toBeNull();
  });
});
