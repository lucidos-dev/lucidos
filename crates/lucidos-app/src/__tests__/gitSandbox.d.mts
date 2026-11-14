export interface Sandbox {
  root: string;
  git(...args: string[]): string;
  write(path: string, text: string): void;
  read(path: string): string;
  remove(): void;
}

export function makeRepo(): Sandbox;
export function readIfPresent(path: string): string | null;
export function writeAt(path: string, text: string): void;
export function tempDir(prefix: string): string;
export function removeDir(path: string): void;
export function envWith(overrides: Record<string, string>): Record<string, string | undefined>;
export function join(...parts: string[]): string;
