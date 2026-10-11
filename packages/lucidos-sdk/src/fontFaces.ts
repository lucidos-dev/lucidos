/**
 * Register a workspace font's faces with the document (ADR 0308).
 *
 * The one DOM half of the workspace font contract; the rules live in the pure
 * `appearance.ts`. A face registers through the CSS Font Loading API, which
 * takes the family and descriptors as values, so nothing is built as CSS text.
 *
 * The caller builds each URL, because each realm reaches `/data` its own way:
 * the shell with its device credential, an app frame with its capability pass.
 * The path is already sanitised to a font file in the font's own directory.
 *
 * Imports only the pure `appearance.ts`, so the shell can reach it without the
 * SDK barrel, and the boot bundles stay dependency-free.
 */

import { registeredFaceWeight, type ResolvedFont, type WorkspaceFont } from './appearance';

/** A face this document registered. `path` is the `data/`-relative file, so
 *  it outlives the expiring URL the face was loaded from. */
export interface RegisteredFace {
  family: string;
  path: string;
  weight: string;
  style: string;
}

/** Faces already registered in this document, by family and path. The boot
 *  script and the SDK or shell are separate bundles in one document. So the
 *  map lives on the global, or each bundle would register the faces again. */
function registeredFaces(): Map<string, RegisteredFace> {
  const holder = globalThis as { __lucidosWorkspaceFaceRegistry?: Map<string, RegisteredFace> };
  holder.__lucidosWorkspaceFaceRegistry ??= new Map();
  return holder.__lucidosWorkspaceFaceRegistry;
}

/** Every workspace font face registered in this document. The `FontFace` API
 *  hides a face's source, and a capture has to embed the file. */
export function registeredWorkspaceFaces(): RegisteredFace[] {
  return [...registeredFaces().values()];
}

/** Add every face of `font` to `document.fonts`, once, and load it at once. A
 *  frame's URL carries a pass that expires, so a face fetched later, when text
 *  first uses it, could be refused. A browser without the API keeps the
 *  fallback stack. */
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
    // Best-effort, with no user intent behind it: a refused face paints the
    // fallback stack, and the next page load registers it again.
    fontFace.load().catch((err) => console.warn(`[lucidos] workspace font face ${face.path} failed to load:`, err));
  }
}

/** Register the workspace fonts a surface paints: its resolved UI font, and
 *  the theme's code font when that is a workspace font. */
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
