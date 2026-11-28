import { SdkError, apiUrl, dataMountUrl, request, requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';
import { wsLocalGet } from './_storage';
import { fromHost, onHostPush, postToHost } from './_bridge';
import {
  PREF_EXTERNAL_LINK_TARGET, PREF_FONT_FAMILY, PREF_MOTION, PREF_THEME, PREF_UI_SCALE,
  type PreferenceValues,
} from './generated/preference-catalog';
import {
  APPEARANCE_CHANNEL, EMPTY_THEME, FONT_FAMILY_STORAGE_KEY,
  THEME_EFFECTS_STORAGE_KEY, THEME_KEY, THEME_SEED_KEY, THEME_STORAGE_KEY, MORE_CONTRAST_QUERY,
  MOTION_STORAGE_KEY, REDUCED_MOTION_QUERY, REDUCED_TRANSPARENCY_QUERY, STYLE_OVERRIDES_STORAGE_KEY,
  FONT_BOLD_ATTRIBUTE, THEME_MODE_ATTRIBUTE, THEME_MODE_BG, THEME_MODE_KEY,
  THEME_MODE_STORAGE_KEY, UI_SCALE_STORAGE_KEY, WORKSPACE_FONT_SEED_KEY,
  followSystemThemeMode, fontBoldMark, fontEntry, isFontId, isWorkspaceFontId, themeBackground, themeEffectsAttribute, themeTokenNames,
  motionAttribute, parseThemeEffects, parseMotion, resolveReducedThemeEffects,
  parseResolvedTheme, parseStyleOverrides, parseUiScale, parseWorkspaceFont, sanitizeAppearancePush,
  PAINTED_THEME_MODE_PUSH_KEY, repaintThemeMode, replaceInlineTokens, resolveFont, resolveThemeMode, sanitizeResolvedTheme, sanitizeWorkspaceFonts,
  resolveReducedMotion, resolveThemeModePreference,
  type FontId, type FontKey, type ThemeEffectsPref, type MotionPref, type ResolvedTheme,
  type ResolvedThemeMode, type ThemeMode, type WorkspaceFont,
} from './appearance';
import { registerFontsInUse } from './fontFaces';
import { servedPrefs } from './boot/appearanceBoot';
import { preferences as prefsModule } from './preferences';
import { inIOSStandalone } from './platform';
import { applyAutocorrectPreference } from './autocorrectStamp';
import { sse } from './sse';
import { Select, enhanceSelects } from './select';
import { ModelPicker } from './modelPicker';
import { disableTooltips } from './tooltip';
import { focusedThread, onFocusedThreadChange } from './focusedThread';
import { pageOpensTabs, pageHandsOffToSafari, openTabNow, openSafariNow, traceAppLink } from './externalLinks';
import type { NavigateTarget, NavigateUi } from './notifications';

export type NavigateParams = Omit<NavigateUi, 'target'>;

export type StartThreadOptions = Pick<NavigateParams, 'prompt' | 'coding_agent' | 'folder'>;

export function webFontUrl(fontKey: FontId): string | undefined {
  return fontEntry(fontKey).source === 'vendored' ? apiUrl(`/fonts/${fontKey}.css`) : undefined;
}

export function webFontUrls(fontKey: FontKey, theme: ResolvedTheme): string[] {
  const keys = new Set<FontId>();
  for (const key of [fontKey, theme.fonts.mono]) if (isFontId(key)) keys.add(key);
  return [...keys].map(webFontUrl).filter((url): url is string => !!url);
}

const loadedFonts = new Set<string>();
let watchingPrefs = false;
let lastThemeModePreference: ThemeMode | null = null;
let lastPaintInput: Record<string, string> | null = null;
let systemThemeModeQuery: MediaQueryList | null = null;
let lastMotionPreference: MotionPref | null = null;
let reducedMotionQuery: MediaQueryList | null = null;
let lastThemeEffectsPreference: ThemeEffectsPref | null = null;
let hostAppearance: Record<string, string> | null = null;
let themeEffectsQueries: MediaQueryList[] = [];

function osPrefersLight(): boolean {
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

function currentThemeModePreference(): ThemeMode {
  return lastThemeModePreference ?? resolveThemeModePreference(
    servedPrefs()?.[THEME_MODE_KEY],
    wsLocalGet(THEME_MODE_STORAGE_KEY),
    () => document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
  );
}

let confirmCounter = 0;
const pendingConfirms = new Map<string, (value: boolean) => void>();
let confirmListenerInstalled = false;

function installConfirmListener() {
  if (confirmListenerInstalled) return;
  confirmListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    if (!fromHost(event)) return;
    const data = event.data as { type?: unknown; id?: unknown; ok?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:confirm:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingConfirms.get(data.id);
    if (!resolver) return;
    pendingConfirms.delete(data.id);
    resolver(data.ok === true);
  });
}

export interface ConfirmOptions {
  title?: string;
  message: string;
  okLabel?: string;
  cancelLabel?: string;
  danger?: boolean;
}

export type ToastType = 'success' | 'info' | 'warning' | 'error';
const TOAST_TYPES: readonly ToastType[] = ['success', 'info', 'warning', 'error'];

export interface ToastOptions {
  title?: string;
  durationMs?: number;
  dismissable?: boolean;
  key?: string;
  spinning?: boolean;
}

export interface PromptOptions {
  message: string;
  title?: string;
  defaultValue?: string;
  placeholder?: string;
  okLabel?: string;
  cancelLabel?: string;
  multiline?: boolean;
}

let promptCounter = 0;
const pendingPrompts = new Map<string, (value: string | null) => void>();
let promptListenerInstalled = false;

function installPromptListener() {
  if (promptListenerInstalled) return;
  promptListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    if (!fromHost(event)) return;
    const data = event.data as { type?: unknown; id?: unknown; value?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:prompt:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingPrompts.get(data.id);
    if (!resolver) return;
    pendingPrompts.delete(data.id);
    resolver(typeof data.value === 'string' ? data.value : null);
  });
}

export interface FilePreviewParams {
  file_path: string;
  line?: number;
  line_end?: number;
}

let previewCounter = 0;
const pendingPreviews = new Map<string, (result: { ok: boolean; error?: string }) => void>();
let previewListenerInstalled = false;

function installPreviewListener() {
  if (previewListenerInstalled) return;
  previewListenerInstalled = true;
  window.addEventListener('message', (event: MessageEvent) => {
    if (!fromHost(event)) return;
    const data = event.data as { type?: unknown; id?: unknown; ok?: unknown; error?: unknown } | null;
    if (!data || typeof data !== 'object') return;
    if (data.type !== 'lucidos:ui:preview-file:result') return;
    if (typeof data.id !== 'string') return;
    const resolver = pendingPreviews.get(data.id);
    if (!resolver) return;
    pendingPreviews.delete(data.id);
    resolver({
      ok: data.ok === true,
      error: typeof data.error === 'string' ? data.error : undefined,
    });
  });
}

export type ExternalLinkTarget = PreferenceValues<'external_link_target'>;

const EXTERNAL_LINK_TARGETS: readonly ExternalLinkTarget[] = PREF_EXTERNAL_LINK_TARGET.values;

let externalLinkTargetCache: ExternalLinkTarget | null = null;

function cacheExternalLinkTarget(raw: string | undefined): void {
  externalLinkTargetCache = EXTERNAL_LINK_TARGETS.includes(raw as ExternalLinkTarget)
    ? raw as ExternalLinkTarget
    : PREF_EXTERNAL_LINK_TARGET.fallback;
}

let devicePreferencesLoaded = false;
let primingDevicePreferences: Promise<void> | null = null;
let devicePreferencesIssued = 0;
let devicePreferencesApplied = 0;

function applyDevicePreferences(prefs: Record<string, string>, attempt: number): void {
  if (attempt < devicePreferencesApplied) return;
  devicePreferencesApplied = attempt;
  devicePreferencesLoaded = true;
  cacheExternalLinkTarget(prefs['external_link_target']);
  applyAutocorrectPreference(prefs);
  if (hostAppearance) return;
  applyMotionPreference(prefs);
  applyThemeEffectsPreference(prefs);
}

function applyMotionPreference(prefs: Record<string, string>): void {
  lastMotionPreference = parseMotion(prefs[PREF_MOTION.key] || wsLocalGet(MOTION_STORAGE_KEY));
  resolveMotionAttribute();
}

function resolveMotionAttribute(): void {
  if (typeof document === 'undefined' || lastMotionPreference === null) return;
  const osReduces = window.matchMedia?.(REDUCED_MOTION_QUERY).matches === true;
  document.documentElement.setAttribute(
    'data-motion', motionAttribute(resolveReducedMotion(lastMotionPreference, osReduces)),
  );
}

function applyThemeEffectsPreference(prefs: Record<string, string>): void {
  lastThemeEffectsPreference = parseThemeEffects(
    prefs['theme-effects'] || wsLocalGet(THEME_EFFECTS_STORAGE_KEY),
  );
  resolveThemeEffectsAttribute();
}

function resolveThemeEffectsAttribute(): void {
  if (typeof document === 'undefined' || lastThemeEffectsPreference === null) return;
  const matches = (query: string) => window.matchMedia?.(query).matches === true;
  const reduced = resolveReducedThemeEffects(
    lastThemeEffectsPreference,
    matches(REDUCED_TRANSPARENCY_QUERY),
    matches(MORE_CONTRAST_QUERY),
  );
  document.documentElement.setAttribute('data-theme-effects', themeEffectsAttribute(reduced));
}

export function primeDevicePreferences(): Promise<void> {
  if (devicePreferencesLoaded) return Promise.resolve();
  if (!primingDevicePreferences) {
    const attempt = ++devicePreferencesIssued;
    primingDevicePreferences = prefsModule.get()
      .then((prefs) => applyDevicePreferences(prefs, attempt))
      .finally(() => { primingDevicePreferences = null; });
  }
  return primingDevicePreferences;
}

const HTTP_SCHEME_RE = /^https?:\/\//i;

let appliedOverrideNames: string[] = [];

function applyStyleOverrides(raw: string | null | undefined, beneath: Record<string, string>): void {
  appliedOverrideNames = replaceInlineTokens(
    document.documentElement.style, appliedOverrideNames, parseStyleOverrides(raw), beneath,
  );
}

let activeTheme: ResolvedTheme = parseResolvedTheme(
  servedPrefs()?.[THEME_SEED_KEY] ?? wsLocalGet(THEME_STORAGE_KEY),
);
let appliedThemeNames: string[] = themeTokenNames(activeTheme);
let themeId: string | null = null;
let themeStale = false;

const seededWorkspaceFont = parseWorkspaceFont(servedPrefs()?.[WORKSPACE_FONT_SEED_KEY]);
let workspaceFonts: WorkspaceFont[] = seededWorkspaceFont ? [seededWorkspaceFont] : [];
let workspaceFontsStale = seededWorkspaceFont === null;

async function fetchWorkspaceFonts(): Promise<WorkspaceFont[] | null> {
  try {
    const listing = await request<{ fonts?: Array<{ source?: unknown }> }>('/fonts');
    return sanitizeWorkspaceFonts((listing?.fonts ?? []).filter(f => f?.source === 'workspace'));
  } catch (err) {
    console.warn('[lucidos-sdk] could not load the workspace fonts:', err);
    return null;
  }
}

async function fetchTheme(id: string): Promise<ResolvedTheme | null> {
  if (id === PREF_THEME.fallback) return EMPTY_THEME;
  try {
    const theme = await request<{ resolved?: unknown }>(`/theme?id=${encodeURIComponent(id)}`);
    return sanitizeResolvedTheme(theme?.resolved);
  } catch (err) {
    if (err instanceof SdkError && (err.httpCode === 404 || err.httpCode === 400)) return EMPTY_THEME;
    console.warn(`[lucidos-sdk] could not load theme "${id}":`, err);
    return null;
  }
}

function paint(prefs: Record<string, string>, freshOsRead = false): void {
  lastPaintInput = prefs;
  const previous = currentThemeModePreference();
  lastThemeModePreference = resolveThemeModePreference(
    prefs[THEME_MODE_KEY],
    wsLocalGet(THEME_MODE_STORAGE_KEY),
    () => document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
  );
  const shellMode = prefs[PAINTED_THEME_MODE_PUSH_KEY];
  let mode: ResolvedThemeMode;
  if (freshOsRead) {
    mode = resolveThemeMode(lastThemeModePreference, osPrefersLight());
  } else if (lastThemeModePreference === 'system' && (shellMode === 'light' || shellMode === 'dark')) {
    mode = shellMode;
  } else {
    mode = repaintThemeMode(
      lastThemeModePreference,
      previous,
      document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
      osPrefersLight,
    );
  }
  const themeTokens = activeTheme[mode];
  const root = document.documentElement;
  for (const name of appliedThemeNames) {
    if (!(name in themeTokens)) root.style.removeProperty(name);
  }
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[mode];
  root.setAttribute(THEME_MODE_ATTRIBUTE, mode);
  root.style.setProperty('--bg-primary', bg);
  root.style.background = bg;
  appliedThemeNames = replaceInlineTokens(root.style, [], themeTokens);
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) root.style.background = 'var(--bg-primary)';

  const known = [...workspaceFonts, ...activeTheme.workspace_fonts];
  const font = resolveFont(storedFontPreference(prefs), activeTheme.fonts, known);
  registerFontsInUse(font, known, activeTheme.fonts.mono, dataMountUrl);
  for (const url of webFontUrls(font.key, activeTheme)) {
    if (loadedFonts.has(url)) continue;
    loadedFonts.add(url);
    const link = document.createElement('link');
    link.rel = 'stylesheet';
    link.href = url;
    document.head.appendChild(link);
  }
  root.style.setProperty('--font-ui', font.stack);
  root.style.setProperty('--font-features-text', font.features.text);
  root.style.setProperty('--font-features-code', font.features.code);
  root.setAttribute(FONT_BOLD_ATTRIBUTE, fontBoldMark(font));

  const scale = parseUiScale(
    prefs[PREF_UI_SCALE.key] || prefs['text-size'] || prefs['font-size']
    || wsLocalGet(UI_SCALE_STORAGE_KEY),
  );
  if (scale !== null) root.style.setProperty('--user-ui-scale', `${scale}%`);

  applyStyleOverrides(prefs['style_overrides'] || wsLocalGet(STYLE_OVERRIDES_STORAGE_KEY), themeTokens);
}

function storedFontPreference(prefs: Record<string, string>): string | null {
  return prefs[PREF_FONT_FAMILY.key] || wsLocalGet(FONT_FAMILY_STORAGE_KEY);
}

function adoptHostAppearance(data: unknown): void {
  const push = sanitizeAppearancePush(data);
  if (!push) return;
  hostAppearance = push;
  activeTheme = parseResolvedTheme(push[THEME_SEED_KEY]);
  const pickedFont = parseWorkspaceFont(push[WORKSPACE_FONT_SEED_KEY]);
  workspaceFonts = pickedFont ? [pickedFont] : [];
  paint(push);
  applyMotionPreference(push);
  applyThemeEffectsPreference(push);
}

export const ui = {
  async applyPreferences(): Promise<void> {
    const attempt = ++devicePreferencesIssued;
    const prefs = await prefsModule.get();
    if (!hostAppearance) {
      const nextThemeId = prefs[THEME_KEY] || PREF_THEME.fallback;
      if (nextThemeId !== themeId || themeStale) {
        const theme = await fetchTheme(nextThemeId);
        if (attempt !== devicePreferencesIssued) return;
        if (theme && !hostAppearance) {
          activeTheme = theme;
          themeId = nextThemeId;
          themeStale = false;
        }
      }
    }
    if (!hostAppearance && isWorkspaceFontId(storedFontPreference(prefs)) && workspaceFontsStale) {
      const listed = await fetchWorkspaceFonts();
      if (attempt !== devicePreferencesIssued) return;
      if (listed && !hostAppearance) {
        workspaceFonts = listed;
        workspaceFontsStale = false;
      }
    }
    if (!hostAppearance) paint(prefs);

    applyDevicePreferences(prefs, attempt);
  },

  watchPreferences(): void {
    if (watchingPrefs) return;
    watchingPrefs = true;
    const reapply = () => {
      ui.applyPreferences().catch((err) => {
        console.warn('[lucidos-sdk] live preference re-apply failed:', err);
      });
    };
    onHostPush(APPEARANCE_CHANNEL, adoptHostAppearance);
    sse.on('PreferencesChanged', (data: unknown) => {
      const key = (data as { key?: unknown } | null)?.key;
      if (key === THEME_KEY) themeStale = true;
      if (key === PREF_FONT_FAMILY.key) workspaceFontsStale = true;
      reapply();
    });
    const refetchTheme = () => {
      themeStale = true;
      if (!hostAppearance) reapply();
    };
    const refetchIfActiveTheme = (data: unknown) => {
      const path = (data as { path?: unknown } | null)?.path;
      if (typeof path === 'string' && path.startsWith('fonts/')) {
        workspaceFontsStale = true;
        refetchTheme();
      } else if (themeId !== null && path === `themes/${themeId}.json`) {
        refetchTheme();
      }
    };
    sse.on('DataFileWritten', refetchIfActiveTheme);
    sse.on('DataFileEdited', refetchIfActiveTheme);
    sse.on('DataFileDeleted', refetchIfActiveTheme);
    const refetchThemeAndFonts = () => {
      workspaceFontsStale = true;
      refetchTheme();
    };
    sse.on('PluginInstalled', refetchThemeAndFonts);
    sse.on('PluginUninstalled', refetchThemeAndFonts);
    sse.connect();
    if (typeof window === 'undefined' || typeof window.matchMedia !== 'function') return;

    const { schedule: scheduleRefresh } = followSystemThemeMode({
      visible: () => document.visibilityState === 'visible',
      moved: () => currentThemeModePreference() === 'system'
        && document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) !== resolveThemeMode('system', osPrefersLight()),
      apply: () => paint(lastPaintInput ?? servedPrefs() ?? {}, true),
    });
    systemThemeModeQuery = window.matchMedia('(prefers-color-scheme: light)');
    systemThemeModeQuery.addEventListener('change', scheduleRefresh);
    document.addEventListener('visibilitychange', scheduleRefresh);
    window.addEventListener('focus', scheduleRefresh);
    window.addEventListener('pageshow', scheduleRefresh);
    reducedMotionQuery = window.matchMedia(REDUCED_MOTION_QUERY);
    reducedMotionQuery.addEventListener('change', resolveMotionAttribute);
    themeEffectsQueries = [REDUCED_TRANSPARENCY_QUERY, MORE_CONTRAST_QUERY].map(q => window.matchMedia(q));
    for (const query of themeEffectsQueries) query.addEventListener('change', resolveThemeEffectsAttribute);
  },

  navigate(target: NavigateTarget, params: NavigateParams = {}): Promise<void> {
    assertPlainObject('params', params);
    return requestVoid('/ui/navigate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, params }),
    });
  },

  openExternal(url: string): Promise<void> {
    assertString('url', url);
    const opens = pageOpensTabs() ? 'tab' : pageHandsOffToSafari() ? 'safari' : null;
    if (opens && HTTP_SCHEME_RE.test(url)) {
      if ((opens === 'tab' ? openTabNow : openSafariNow)(url)) {
        traceAppLink('frame-opened', { href: url, opens });
        return Promise.resolve();
      }
      traceAppLink('frame-open-blocked', { href: url, opens });
    }
    if (
      inIOSStandalone()
      && HTTP_SCHEME_RE.test(url)
      && externalLinkTargetCache === 'ask'
      && typeof navigator.share === 'function'
    ) {
      return navigator.share({ url }).catch((err: unknown) => {
        if (err instanceof Error && err.name === 'AbortError') return;
        return ui.navigate('url', { url });
      });
    }
    return ui.navigate('url', { url });
  },

  startThread(opts?: StartThreadOptions): Promise<void> {
    const given: Record<string, string> = {};
    for (const key of ['prompt', 'coding_agent', 'folder'] as const) {
      const value = opts?.[key];
      if (value === undefined) continue;
      if (typeof value !== 'string') {
        return Promise.reject(new TypeError(`opts.${key} must be a string`));
      }
      if (value.length > 0) given[key] = value;
    }
    return ui.navigate('new-chat', given as NavigateParams);
  },

  confirm(options: ConfirmOptions): Promise<boolean> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    if (window.parent === window) {
      return Promise.resolve(window.confirm(options.message));
    }
    installConfirmListener();
    const id = `c${++confirmCounter}-${Date.now()}`;
    const payload = {
      title: typeof options.title === 'string' ? options.title : undefined,
      message: options.message,
      okLabel: typeof options.okLabel === 'string' && options.okLabel.length > 0 ? options.okLabel : 'Confirm',
      cancelLabel: typeof options.cancelLabel === 'string' && options.cancelLabel.length > 0 ? options.cancelLabel : 'Cancel',
      danger: options.danger === true,
    };
    return new Promise<boolean>((resolve) => {
      pendingConfirms.set(id, resolve);
      postToHost({ type: 'lucidos:ui:confirm', id, payload });
    });
  },

  toast(message: string, type: ToastType = 'info', opts?: ToastOptions): void {
    if (typeof message !== 'string' || message.length === 0) {
      throw new TypeError('lucidos.ui.toast: message must be a non-empty string');
    }
    if (opts?.title !== undefined && typeof opts.title !== 'string') {
      throw new TypeError('lucidos.ui.toast: opts.title must be a string');
    }
    const safeType: ToastType = TOAST_TYPES.includes(type) ? type : 'info';
    const payload = {
      message,
      type: safeType,
      title: opts?.title ? opts.title : undefined,
      durationMs: opts && typeof opts.durationMs === 'number' ? opts.durationMs : undefined,
      dismissable: opts && typeof opts.dismissable === 'boolean' ? opts.dismissable : undefined,
      key: opts && typeof opts.key === 'string' && opts.key.length > 0 ? opts.key : undefined,
      spinning: opts && typeof opts.spinning === 'boolean' ? opts.spinning : undefined,
    };
    if (window.parent === window) {
      const line = `[lucidos.ui.toast:${safeType}] ${payload.title ? `${payload.title}: ` : ''}${message}`;
      if (safeType === 'error') console.error(line);
      else if (safeType === 'warning') console.warn(line);
      else console.log(line);
      return;
    }
    postToHost({ type: 'lucidos:ui:toast', payload });
  },

  dismissToast(key: string): void {
    if (typeof key !== 'string' || key.length === 0) {
      throw new TypeError('lucidos.ui.dismissToast: key must be a non-empty string');
    }
    if (window.parent === window) {
      console.log(`[lucidos.ui.dismissToast] ${key}`);
      return;
    }
    postToHost({ type: 'lucidos:ui:dismissToast', payload: { key } });
  },

  ready(): void {
    if (window.parent === window) return;
    postToHost({ type: 'lucidos:ui:ready' });
  },

  prompt(options: PromptOptions): Promise<string | null> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    if (window.parent === window) {
      const def = typeof options.defaultValue === 'string' ? options.defaultValue : '';
      return Promise.resolve(window.prompt(options.message, def));
    }
    installPromptListener();
    const id = `p${++promptCounter}-${Date.now()}`;
    const payload = {
      title: typeof options.title === 'string' ? options.title : undefined,
      message: options.message,
      defaultValue: typeof options.defaultValue === 'string' ? options.defaultValue : undefined,
      placeholder: typeof options.placeholder === 'string' ? options.placeholder : undefined,
      okLabel: typeof options.okLabel === 'string' && options.okLabel.length > 0 ? options.okLabel : 'OK',
      cancelLabel: typeof options.cancelLabel === 'string' && options.cancelLabel.length > 0 ? options.cancelLabel : 'Cancel',
      multiline: options.multiline === true,
    };
    return new Promise<string | null>((resolve) => {
      pendingPrompts.set(id, resolve);
      postToHost({ type: 'lucidos:ui:prompt', id, payload });
    });
  },

  previewFile(params: FilePreviewParams): Promise<void> {
    assertPlainObject('params', params);
    if (typeof params.file_path !== 'string' || params.file_path.length === 0) {
      return Promise.reject(new TypeError('params.file_path must be a non-empty string'));
    }
    const payload = {
      file_path: params.file_path,
      line: typeof params.line === 'number' ? params.line : undefined,
      line_end: typeof params.line_end === 'number' ? params.line_end : undefined,
    };
    if (window.parent === window) {
      return Promise.reject(new Error(
        'lucidos.ui.previewFile: no host to show the preview (this app is not running inside Lucidos)',
      ));
    }
    installPreviewListener();
    const id = `v${++previewCounter}-${Date.now()}`;
    return new Promise<void>((resolve, reject) => {
      const timeout = setTimeout(() => {
        if (pendingPreviews.delete(id)) {
          reject(new Error('lucidos.ui.previewFile: the host did not respond'));
        }
      }, 60_000);
      pendingPreviews.set(id, (result) => {
        clearTimeout(timeout);
        if (result.ok) resolve();
        else reject(new Error(result.error || 'lucidos.ui.previewFile: the host refused the preview'));
      });
      postToHost({ type: 'lucidos:ui:preview-file', id, payload });
    });
  },

  focusedThread,

  onFocusedThreadChange,

  Select,
  ModelPicker,

  enhanceSelects,

  disableTooltips,
};
