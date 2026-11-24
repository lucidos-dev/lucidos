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

export function refreshWorkspaceFontsIfLoaded(): void {
  if (workspaceFontList.value.status !== 'not-loaded') void loadWorkspaceFonts();
}

export interface FontFileChoice {
  file: File;
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

export function slugFromLabel(label: string): string {
  return label
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .slice(0, MAX_SLUG)
    .replace(/^-+|-+$/g, '');
}

function freeSlug(base: string, taken: ReadonlySet<string>): string {
  if (!taken.has(base)) return base;
  for (let n = 2; ; n++) {
    const suffix = `-${n}`;
    const candidate = `${base.slice(0, MAX_SLUG - suffix.length).replace(/-+$/, '')}${suffix}`;
    if (!taken.has(candidate)) return candidate;
  }
}

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

async function takenSlugs(): Promise<Set<string>> {
  await loadWorkspaceFonts();
  const list = workspaceFontList.value;
  if (list.status !== 'loaded') {
    throw new Error('the installed fonts could not be read, so a new one could overwrite them');
  }
  const ids = [...list.data.fonts.map(font => font.id), ...list.data.invalid.map(font => font.id)];
  return new Set(ids.map(id => id.slice(WORKSPACE_FONT_ID_PREFIX.length)));
}

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

export function guessFace(fileName: string): { weight: string; style: 'normal' | 'italic' } {
  const stem = fileName.replace(/\.[^.]+$/, '');
  const style = /italic|oblique/i.test(stem) ? 'italic' : 'normal';
  if (/variable|[-_]vf\b|\[wght/i.test(stem)) return { weight: '100 900', style };
  const named = WEIGHT_NAMES.find(([pattern]) => pattern.test(stem));
  return { weight: named ? named[1] : '400', style };
}
