import { mkdir, readFile, writeFile } from 'node:fs/promises';
import { dirname } from 'node:path';

export async function writeIfChanged(path, text) {
  const current = await readFile(path, 'utf8').catch(() => null);
  if (current === text) return;
  await mkdir(dirname(path), { recursive: true });
  await writeFile(path, text, 'utf8');
}
