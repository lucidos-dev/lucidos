
import { registeredFaceWeight, type ResolvedFont, type WorkspaceFont } from './appearance';

export interface RegisteredFace {
  family: string;
  path: string;
  weight: string;
  style: string;
}

function registeredFaces(): Map<string, RegisteredFace> {
  const holder = globalThis as { __lucidosWorkspaceFaceRegistry?: Map<string, RegisteredFace> };
  holder.__lucidosWorkspaceFaceRegistry ??= new Map();
  return holder.__lucidosWorkspaceFaceRegistry;
}

export function registeredWorkspaceFaces(): RegisteredFace[] {
  return [...registeredFaces().values()];
}

export function registerWorkspaceFont(font: WorkspaceFont, urlFor: (path: string) => string): void {
  if (typeof FontFace === 'undefined' || typeof document === 'undefined' || !document.fonts) return;
  const registered = registeredFaces();
  for (const face of font.faces) {
    const key = `${font.family}\n${face.path}`;
    if (registered.has(key)) continue;
    const weight = registeredFaceWeight(font, face);
    registered.set(key, { family: font.family, path: face.path, weight, style: face.style });
    const fontFace = new FontFace(font.family, `url("${urlFor(face.path)}")`, {
      weight,
      style: face.style,
      display: 'swap',
    });
    document.fonts.add(fontFace);
    fontFace.load().catch((err) => console.warn(`[lucidos] workspace font face ${face.path} failed to load:`, err));
  }
}

export function registerFontsInUse(
  font: ResolvedFont,
  known: readonly WorkspaceFont[],
  monoId: string | undefined,
  urlFor: (path: string) => string,
): void {
  for (const entry of known) {
    if (entry === font.workspaceFont || entry.id === monoId) registerWorkspaceFont(entry, urlFor);
  }
}
