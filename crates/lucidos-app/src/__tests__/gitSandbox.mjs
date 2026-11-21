// Throwaway git repositories for tests that need real history. Plain JS, so a
// test reaches git and the file system through typed helpers (gitSandbox.d.mts).

import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';

/** A fresh repository on `main`, with an identity and `.lucidos/` ignored. */
export function makeRepo() {
  const root = mkdtempSync(join(tmpdir(), 'git-sandbox-'));
  const git = (...args) => execFileSync('git', args, { cwd: root, encoding: 'utf8' }).trim();
  git('init', '-q', '-b', 'main');
  git('config', 'user.email', 'test@example.com');
  git('config', 'user.name', 'Test');
  writeFileSync(join(root, '.gitignore'), '.lucidos/\n');
  writeFileSync(join(root, 'package.json'), '{}');
  return {
    root,
    git,
    write(path, text) {
      mkdirSync(dirname(join(root, path)), { recursive: true });
      writeFileSync(join(root, path), text);
    },
    read: (path) => readFileSync(join(root, path), 'utf8'),
    remove: () => removeDir(root),
  };
}

export function readIfPresent(path) {
  return existsSync(path) ? readFileSync(path, 'utf8') : null;
}

export function writeAt(path, text) {
  writeFileSync(path, text);
}

export function tempDir(prefix) {
  return mkdtempSync(join(tmpdir(), prefix));
}

export function removeDir(path) {
  rmSync(path, { recursive: true, force: true });
}

/** This process's environment with `overrides` on top, for a child process. */
export function envWith(overrides) {
  return { ...process.env, ...overrides };
}

export { join };
