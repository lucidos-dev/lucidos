import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';


export const DESKTOP_IGNORES = /-mobile\.spec\.ts$/;

export const PHONE_SPECS = /-(mobile|all)\.spec\.ts$/;

export const DESKTOP_WEBKIT_SPECS: readonly string[] = readFileSync(
  fileURLToPath(new URL('./desktop-webkit-specs.txt', import.meta.url)),
  'utf8',
)
  .split('\n')
  .map(line => line.trim())
  .filter(line => line !== '' && !line.startsWith('#'));
