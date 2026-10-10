/**
 * Workspace fonts in the shell: the installed list, and installing or removing
 * one from Settings (ADR 0308).
 *
 * A workspace font is a directory `data/fonts/<slug>/`: the font files, then a
 * `font.json` naming them. Install writes the files first and the manifest last,
 * so a font appears only once it is complete. The engine checks every write,
 * and every font again when it lists them. So this module shows the engine's
 * reasons rather than validating on its own.
 *
 * A leaf: `preferences.ts` reads the list to paint a picked font, and nothing
 * here imports the preference store.
 */
import { signal } from '@preact/signals';
import {
  WORKSPACE_FONT_ID_PREFIX,
  WORKSPACE_FONT_LIMITS,
  sanitizeWorkspaceFonts,
  type FontGroup,
  type WorkspaceFont,
  type WorkspaceFontId,
} from '@lucidos/appearance';
import {
  deleteFontFile,
  listFontFiles,
  listFonts,
  writeFontFile,
  type InvalidWorkspaceFont,
} from '../../api/client';
import { failedIfFresh, setLoadingIfFresh, type Loadable } from '../types';

export interface WorkspaceFontList {
  fonts: WorkspaceFont[];
  invalid: InvalidWorkspaceFont[];
}

export const workspaceFontList = signal<Loadable<WorkspaceFontList>>({ status: 'not-loaded' });

let loadSeq = 0;

export async function loadWorkspaceFonts(): Promise<void> {
  const seq = ++loadSeq;
  setLoadingIfFresh(workspaceFontList);
  try {
    const listing = await listFonts();
    if (seq !== loadSeq) return;
    const fonts = sanitizeWorkspaceFonts(listing.fonts.filter(font => font.source === 'workspace'));
    workspaceFontList.value = { status: 'loaded', data: { fonts, invalid: listing.invalid ?? [] } };
  } catch (e) {
    if (seq !== loadSeq) return;
    workspaceFontList.value = failedIfFresh(workspaceFontList.value, e);
  }
}

/** Reload the list only if something already asked for it: the shell loads it
 *  with the preferences, so in practice this always reloads. */
export function refreshWorkspaceFontsIfLoaded(): void {
  if (workspaceFontList.value.status !== 'not-loaded') void loadWorkspaceFonts();
}

/** One font file to install, with the descriptors its face registers under. */
export interface FontFileChoice {
  file: File;
  /** One weight, or the `min max` range of a variable file. */
  weight: string;
  style: 'normal' | 'italic';
}

export interface FontInstall {
  label: string;
  group: FontGroup;
  ligatures: boolean;
  files: FontFileChoice[];
}

const MAX_SLUG = WORKSPACE_FONT_LIMITS.slug;

/** A directory name from a label: lowercase letters and digits joined by
 *  single hyphens, as the engine requires. Empty when nothing usable is left. */
export function slugFromLabel(label: string): string {
  return label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .slice(0, MAX_SLUG)
    .replace(/^-+|-+$/g, '');
}

/** A slug no installed or broken font already uses. */
function freeSlug(base: string, taken: ReadonlySet<string>): string {
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) {
    const suffix = `-${n}`;
    const candidate = `${base.slice(0, MAX_SLUG - suffix.length).replace(/-+$/, '')}${suffix}`;
    if (!taken.has(candidate)) return candidate;
  }
}

/** A file name the engine accepts: letters, digits, `.`, `_` and `-`, not
 *  starting with a dot, with a lowercase extension. Made unique per font. */
export function safeFileName(name: string, taken: Set<string>): string {
  const dot = name.lastIndexOf('.');
  const ext = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';
  const stem = (dot > 0 ? name.slice(0, dot) : name)
    .replace(/[^A-Za-z0-9_-]+/g, '-')
    .replace(/^[-.]+/, '')
    .slice(0, 80) || 'font';
  let candidate = `${stem}.${ext}`;
  for (let n = 2; taken.has(candidate); n++) candidate = `${stem}-${n}.${ext}`;
  taken.add(candidate);
  return candidate;
}

/** Every directory an installed or broken font holds, from a list read just
 *  now. An install must never pick one of them: its writes would overwrite
 *  that font, and a failed install's cleanup would then delete its files. */
async function takenSlugs(): Promise<Set<string>> {
  await loadWorkspaceFonts();
  const list = workspaceFontList.value;
  if (list.status !== 'loaded') {
    throw new Error('the installed fonts could not be read, so a new one could overwrite them');
  }
  const ids = [...list.data.fonts.map(font => font.id), ...list.data.invalid.map(font => font.id)];
  return new Set(ids.map(id => id.slice(WORKSPACE_FONT_ID_PREFIX.length)));
}

/**
 * Install a font: every file, then the manifest. Answers the new font's id. A
 * refused write throws the engine's reason, and removes what this call wrote,
 * so a failed install leaves nothing half-made behind.
 */
export async function installWorkspaceFont(install: FontInstall): Promise<WorkspaceFontId> {
  const base = slugFromLabel(install.label);
  if (!base) throw new Error('Give the font a name with at least one letter or digit.');
  if (install.files.length === 0) throw new Error('Choose at least one font file.');
  const slug = freeSlug(base, await takenSlugs());
  const names = new Set<string>();
  const faces = install.files.map(choice => ({
    choice,
    file: safeFileName(choice.file.name, names),
  }));
  const manifest = {
    label: install.label.trim(),
    group: install.group,
    ligatures: install.group === 'mono' && install.ligatures,
    faces: faces.map(({ choice, file }) => ({ file, weight: choice.weight, style: choice.style })),
  };

  const written: string[] = [];
  try {
    for (const { choice, file } of faces) {
      const path = `fonts/${slug}/${file}`;
      await writeFontFile(path, choice.file);
      written.push(path);
    }
    await writeFontFile(`fonts/${slug}/font.json`, JSON.stringify(manifest, null, 2));
  } catch (e) {
    await Promise.allSettled(written.map(deleteFontFile));
    throw e;
  } finally {
    void loadWorkspaceFonts();
  }
  return `${WORKSPACE_FONT_ID_PREFIX}${slug}` as WorkspaceFontId;
}

/** Remove a font, broken or not: every file in its directory, nested ones
 *  included, or a leftover would keep it listed. The manifest goes first, so
 *  the font leaves the list before its files do. A broken font's id holds its
 *  raw directory name, so files are matched by exact prefix, never by a glob
 *  built from it. */
export async function removeWorkspaceFont(id: string): Promise<void> {
  const dir = `fonts/${id.slice(WORKSPACE_FONT_ID_PREFIX.length)}/`;
  const files = (await listFontFiles()).filter(path => path.startsWith(dir));
  const manifest = `${dir}font.json`;
  const ordered = [...files.filter(f => f === manifest), ...files.filter(f => f !== manifest)];
  try {
    for (const path of ordered) await deleteFontFile(path);
  } finally {
    void loadWorkspaceFonts();
  }
}

/** Weight names as font files spell them, compound spellings first so
 *  `ExtraBold` is not read as `Bold`. */
const WEIGHT_NAMES: Array<[RegExp, string]> = [
  [/extra-?light|ultra-?light/i, '200'],
  [/semi-?bold|demi-?bold/i, '600'],
  [/extra-?bold|ultra-?bold/i, '800'],
  [/thin|hairline/i, '100'],
  [/light/i, '300'],
  [/medium/i, '500'],
  [/bold/i, '700'],
  [/black|heavy/i, '900'],
];

/** A first guess at a file's weight and style from its name, for the install
 *  form to prefill. A variable file covers the whole range. */
export function guessFace(fileName: string): { weight: string; style: 'normal' | 'italic' } {
  const stem = fileName.replace(/\.[^.]+$/, '');
  const style = /italic|oblique/i.test(stem) ? 'italic' : 'normal';
  if (/variable|[-_]vf\b|\[wght/i.test(stem)) return { weight: '100 900', style };
  const named = WEIGHT_NAMES.find(([pattern]) => pattern.test(stem));
  return { weight: named ? named[1] : '400', style };
}
