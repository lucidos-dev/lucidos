
import { dataMountUrl, getBaseUrl, requestBlob, requestText } from './_fetch';
import { splitCapability } from './frameCapability';
import { registeredWorkspaceFaces } from './fontFaces';

export type Inliner = (url: string) => Promise<string | null>;

export function bridgePathFor(url: string, baseUrl: string, documentUrl: string): string | null {
  let target: URL;
  let root: URL;
  try {
    target = new URL(url, documentUrl);
    root = new URL(baseUrl || '/', documentUrl);
  } catch {
    return null;
  }
  if (target.origin !== root.origin) return null;
  const prefix = root.pathname.replace(/\/$/, '');
  if (!target.pathname.startsWith(`${prefix}/`)) return null;
  const unprefixed = target.pathname.slice(prefix.length);
  const path = splitCapability(unprefixed)?.rest ?? unprefixed;

  if (path.startsWith('/api/v1/')) return path.slice('/api/v1'.length) + target.search;
  if (path.startsWith('/data/')) return path;
  const app = /^\/app\/([^/]+)\/(.+)$/.exec(path);
  if (!app || target.searchParams.has('thread_id')) return null;
  const [, appId, rest] = app;
  return rest.startsWith('artifacts/') ? `/data/${rest}` : `/data/apps/${appId}/${rest}`;
}

function blobToDataUrl(blob: Blob): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(reader.result as string);
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(blob);
  });
}

export const EMBED_BUDGET_MS = 3000;

function timeLeft(until: number): number {
  return Math.max(1, until - performance.now());
}

export function createInliner(until: number): Inliner {
  const cache = new Map<string, Promise<string | null>>();
  const read = async (url: string): Promise<string | null> => {
    if (url.startsWith('data:')) return url;
    try {
      if (url.startsWith('blob:')) return await blobToDataUrl(await (await fetch(url)).blob());
      const path = bridgePathFor(url, getBaseUrl(), document.baseURI);
      if (!path) return null;
      return await blobToDataUrl(await requestBlob(path, undefined, timeLeft(until)));
    } catch {
      return null;
    }
  };
  return (url) => {
    let hit = cache.get(url);
    if (!hit) {
      hit = read(url);
      cache.set(url, hit);
    }
    return hit;
  };
}

const CSS_URL = /url\(\s*(?:"((?:[^"\\]|\\.)*)"|'((?:[^'\\]|\\.)*)'|([^'")\s]+))\s*\)/g;

function urlOf(match: RegExpMatchArray | string[]): string {
  return (match[1] ?? match[2] ?? match[3] ?? '').replace(/\\(.)/g, '$1');
}

export async function inlineCssUrls(css: string, base: string, inline: Inliner): Promise<string> {
  const urls = new Set<string>();
  for (const match of css.matchAll(CSS_URL)) {
    const url = urlOf(match);
    if (!url.startsWith('data:')) urls.add(url);
  }
  if (urls.size === 0) return css;
  const resolved = new Map<string, string | null>();
  await Promise.all([...urls].map(async (raw) => {
    let absolute: string;
    try {
      absolute = new URL(raw, base).href;
    } catch {
      resolved.set(raw, null);
      return;
    }
    resolved.set(raw, await inline(absolute));
  }));
  return css.replace(CSS_URL, (whole: string, ...groups: string[]) => {
    const url = urlOf([whole, ...groups]);
    if (url.startsWith('data:')) return whole;
    const data = resolved.get(url);
    return data ? `url("${data}")` : 'url("")';
  });
}

export function fontFamilies(value: string): string[] {
  return value
    .split(',')
    .map((name) => name.trim().replace(/^(['"])(.*)\1$/, '$2').trim().toLowerCase())
    .filter(Boolean);
}

interface FaceSource {
  css: string;
  base: string;
}

const FONT_FACE_BLOCK = /@font-face\s*\{[^}]*\}/gi;

export function faceFamily(css: string): string | null {
  const match = /font-family\s*:\s*([^;}]+)/i.exec(css);
  return match ? (fontFamilies(match[1])[0] ?? null) : null;
}

interface FaceScan {
  out: FaceSource[];
  pending: Promise<void>[];
  until: number;
}

function collectRules(rules: CSSRuleList, base: string, scan: FaceScan): void {
  for (const rule of Array.from(rules)) {
    if (rule instanceof CSSFontFaceRule) scan.out.push({ css: rule.cssText, base });
    else if (rule instanceof CSSImportRule && rule.styleSheet) sheetFaces(rule.styleSheet, scan);
  }
}

function sheetFaces(sheet: CSSStyleSheet, scan: FaceScan): void {
  const base = sheet.href ?? document.baseURI;
  let rules: CSSRuleList | null = null;
  try {
    rules = sheet.cssRules;
  } catch {
    rules = null;
  }
  if (rules) {
    collectRules(rules, base, scan);
    return;
  }
  const path = sheet.href ? bridgePathFor(sheet.href, getBaseUrl(), document.baseURI) : null;
  if (!path) return;
  scan.pending.push(requestText(path, undefined, timeLeft(scan.until))
    .then((text) => {
      for (const css of text.match(FONT_FACE_BLOCK) ?? []) scan.out.push({ css, base });
    })
    .catch(() => {  }));
}

async function documentFaces(until: number): Promise<FaceSource[]> {
  const scan: FaceScan = { out: [], pending: [], until };
  for (const sheet of Array.from(document.styleSheets)) sheetFaces(sheet, scan);
  await Promise.all(scan.pending);
  const { out } = scan;
  for (const face of registeredWorkspaceFaces()) {
    out.push({
      css: `@font-face{font-family:${JSON.stringify(face.family)};src:url("${dataMountUrl(face.path)}");`
        + `font-weight:${face.weight};font-style:${face.style}}`,
      base: document.baseURI,
    });
  }
  return out;
}

export async function embeddedFontFaces(
  used: ReadonlySet<string>,
  inline: Inliner,
  until: number,
): Promise<string> {
  const faces = (await documentFaces(until)).filter((face) => {
    const family = faceFamily(face.css);
    return family !== null && used.has(family);
  });
  const embedded = await Promise.all(faces.map(async (face) => {
    const css = await inlineCssUrls(face.css, face.base, inline);
    return css.includes('url("")') ? '' : css;
  }));
  return embedded.filter(Boolean).join('\n');
}
