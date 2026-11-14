/**
 * Write a build output only when its bytes changed, so an unchanged file keeps
 * its mtime. Vite watches what it imports, and a rewrite of identical bytes
 * would rebuild or reload for nothing.
 */
import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

export async function writeIfChanged(path, text) {
  const current = await readFile(path, 'utf8').catch(() => null);
  if (current === text) return;
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, text, 'utf8');
}
