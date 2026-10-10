import { SdkError, apiUrl, dataMountUrl, request, requestVoid } from './_fetch';
import { assertPlainObject, assertString } from './_validate';
import { wsLocalGet } from './_storage';
import { onHostPush } from './_bridge';
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
  replaceInlineTokens, resolveFont, resolveThemeMode, sanitizeResolvedTheme, sanitizeWorkspaceFonts,
  resolveReducedMotion, resolveThemeModePreference,
  type FontId, type FontKey, type ThemeEffectsPref, type MotionPref, type ResolvedTheme,
  type ThemeMode, type WorkspaceFont,
} from './appearance';
import { registerFontsInUse } from './fontFaces';
import { servedPrefs } from './boot/appearanceBoot';
import { preferences as prefsModule } from './preferences';
import { isIOSAgent } from './platform';
import { applyAutocorrectPreference } from './autocorrectStamp';
import { sse } from './sse';
import { Select, enhanceSelects } from './select';
import { disableTooltips } from './tooltip';
import type { NavigateTarget, NavigateUi } from './notifications';

/** Params for `lucidos.ui.navigate`: the `NavigateUi` payload minus `target`,
 *  which is the first argument. Carries the generated `settings_view`, so the
 *  full Settings sub-section set is type-checked and discoverable from the SDK,
 *  in lockstep with the engine `navigate_ui` tool. */
export type NavigateParams = Omit<NavigateUi, 'target'>;

/** The stylesheet to load for a font, or `undefined` for a font already on the
 *  device.
 *
 *  Every other font is vendored and resolves to the LOCAL engine, so it works
 *  offline and tells no third party anything (`crates/lucidos-engine/src/api/fonts.rs`,
 *  ADR 0303). Resolved per call rather than held in a map, because `apiUrl`
 *  reads a base URL that `configure({ baseUrl })` can still change. */
export function webFontUrl(fontKey: FontId): string | undefined {
  return fontEntry(fontKey).source === 'vendored' ? apiUrl(`/fonts/${fontKey}.css`) : undefined;
}

/** The stylesheets a frame needs: its UI font's and the theme's code font's.
 *  A workspace font has none: its faces register directly (`fontFaces.ts`). */
export function webFontUrls(fontKey: FontKey, theme: ResolvedTheme): string[] {
  const keys = new Set<FontId>();
  for (const key of [fontKey, theme.fonts.mono]) if (isFontId(key)) keys.add(key);
  return [...keys].map(webFontUrl).filter((url): url is string => !!url);
}

const loadedFonts = new Set<string>();
let watchingPrefs = false;
/** The theme mode PREFERENCE the last `applyPreferences()` settled on, which is what
 *  says whether this frame follows the OS at all. `null` until the first run. */
let lastThemeModePreference: ThemeMode | null = null;
/** Kept alive for as long as its listener must be. See `watchPreferences`. */
let systemThemeModeQuery: MediaQueryList | null = null;
/** The motion preference at the last apply, so an OS flip can re-resolve it. */
let lastMotionPreference: MotionPref | null = null;
/** Held for the same WebKit reason as `systemThemeModeQuery`. */
let reducedMotionQuery: MediaQueryList | null = null;
/** The theme-effects preference at the last apply, for the same reason. */
let lastThemeEffectsPreference: ThemeEffectsPref | null = null;
/** The shell's last appearance push. Once one arrives it owns this frame's
 *  appearance, and a fetch supplies only the behaviour preferences. A fetch can
 *  lag a debounced save, so painting it would undo what the shell just pushed. */
let hostAppearance: Record<string, string> | null = null;
/** The two OS signals `system` theme effects follow, held like the others. */
let themeEffectsQueries: MediaQueryList[] = [];

/** Whether the OS is asking for light right now. */
function osPrefersLight(): boolean {
  return window.matchMedia('(prefers-color-scheme: light)').matches;
}

/** Which theme mode preference this frame is on.
 *
 *  Before the first `applyPreferences()` resolves, fall back to the precedence
 *  the boot script already used. An app may call `watchPreferences()` first, or
 *  not await the fetch, and an OS flip in that window must not be dropped. */
function currentThemeModePreference(): ThemeMode {
  return lastThemeModePreference ?? resolveThemeModePreference(
    undefined,
    wsLocalGet(THEME_MODE_STORAGE_KEY),
    () => document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
  );
}

/** Only the window this frame posted to may answer it.
 *
 *  Every request below goes to `window.parent`, so a reply from anywhere else
 *  is a forgery. A nested iframe an app embeds can post to that app's frame,
 *  and the ids here are a counter plus a timestamp. Without this check such a
 *  frame could resolve a pending `confirm` as OK, with no dialog ever shown.
 *
 *  The host guards the request direction the same way, and against the same
 *  threat (`isKnownAppFrame` in `store/startup.ts`). This is the reply
 *  half. */
function fromHost(event: MessageEvent): boolean {
  return event.source === window.parent;
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

/** Toast severity — matches the host shell's toast types. */
export type ToastType = 'success' | 'info' | 'warning' | 'error';
const TOAST_TYPES: readonly ToastType[] = ['success', 'info', 'warning', 'error'];

export interface ToastOptions {
  /** A bold line over the message. Only the title is bold; without one the
   *  toast is its message alone. A newline in the message is a line break,
   *  and never makes a title. */
  title?: string;
  /** Auto-dismiss after this many ms. Omit for the host default: an error or
   *  warning stays until dismissed, success and info auto-close. */
  durationMs?: number;
  /** false = never show the close (X) button. A toast that leaves on its own
   *  timer and has no button shows none either way. Default true. */
  dismissable?: boolean;
  /** Stable key for in-place replacement. A later toast with the same key
   *  updates the existing toast instead of stacking a new one, so an
   *  'Opening...' toast can become 'Opened'. */
  key?: string;
  /** true = show an indeterminate "work in progress" spinner in place of the
   *  severity icon. Pair it with a `key` so a later keyed toast replaces the
   *  spinning one with the outcome (or call `dismissToast(key)` when the work
   *  finishes with nothing to say). Indeterminate only: there is no app-facing
   *  percentage. */
  spinning?: boolean;
}

export interface PromptOptions {
  /** Required. The question/instruction shown above the input. Plain text. */
  message: string;
  /** Optional heading rendered above the message. */
  title?: string;
  /** Prefilled input value. */
  defaultValue?: string;
  /** Placeholder shown when the input is empty. */
  placeholder?: string;
  /** OK button label. Default "OK". */
  okLabel?: string;
  /** Cancel button label. Default "Cancel". */
  cancelLabel?: string;
  /** Render a multi-line textarea instead of a single-line input. Default false. */
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
    // A string is an OK with the entered text; anything else (cancel/esc) is null.
    resolver(typeof data.value === 'string' ? data.value : null);
  });
}

/** What `lucidos.ui.previewFile` shows.
 *
 *  Deliberately the `file` navigate target's own field names, `snake_case`
 *  included, so one object literal drives both calls and an app promotes a
 *  glance into a navigation by swapping the verb:
 *
 *  ```js
 *  const at = { file_path: `repo:${repoId}:file:src/main.rs`, line: 510, line_end: 520 };
 *  await lucidos.ui.previewFile(at);        // glance, your app stays put
 *  await lucidos.ui.navigate('file', at);   // leave for the Files panel
 *  ```
 */
export interface FilePreviewParams {
  /** The same forms `navigate('file', …)` accepts: a workspace data path
   *  (`artifacts/…`, `knowhow/…`, `apps/…`, `triggers/…`, `system-knowhow/…`, or
   *  a bare name, which is treated as an artifact), or a repo-encoded path for a
   *  file in a registered repository clone, either at its current `HEAD`
   *  (`repo:<repoId>:file:<repo-relative path>`) or at a branch, tag or sha you
   *  name (`repo:<repoId>:file#<ref>:<repo-relative path>`).
   *
   *  Naming the revision matters here: the modal may be showing a repository the
   *  Files panel is not bound to, so it cannot fall back to whatever branch that
   *  panel is on. A file a coding agent has edited is on that agent's branch,
   *  not at `HEAD`. */
  file_path: string;
  /** 1-based first line to highlight and scroll to. */
  line?: number;
  /** Inclusive last line of the range; omit to highlight a single line. */
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

/** Where an external http(s) link goes when tapped in an installed iOS PWA. */
export type ExternalLinkTarget = PreferenceValues<'external_link_target'>;

const EXTERNAL_LINK_TARGETS: readonly ExternalLinkTarget[] = PREF_EXTERNAL_LINK_TARGET.values;

/** The last-seen `external_link_target`, refreshed by `applyPreferences` (and
 *  therefore by `watchPreferences`, which re-runs it on `PreferencesChanged`).
 *
 *  It is a cache rather than a fetch because `openExternal` must decide
 *  SYNCHRONOUSLY: the `ask` mode calls `navigator.share`, which requires
 *  transient user activation, and any `await` between the click and the call
 *  spends it. `null` means "not fetched yet", which routes to the host instead
 *  of guessing. */
let externalLinkTargetCache: ExternalLinkTarget | null = null;

function cacheExternalLinkTarget(raw: string | undefined): void {
  externalLinkTargetCache = EXTERNAL_LINK_TARGETS.includes(raw as ExternalLinkTarget)
    ? raw as ExternalLinkTarget
    : PREF_EXTERNAL_LINK_TARGET.fallback;
}

/** Whether a device-preference read has landed here, from the load-time prime
 *  or from `applyPreferences`. */
let devicePreferencesLoaded = false;
/** The prime's own read, so repeated prime calls share one fetch. */
let primingDevicePreferences: Promise<void> | null = null;
/** Every device-preference read takes the next attempt number when issued. */
let devicePreferencesIssued = 0;
/** The newest attempt applied so far. */
let devicePreferencesApplied = 0;

/**
 * Apply the device preferences every app frame follows, themed or not.
 *
 * The prime and `applyPreferences` each fetch, and never share one promise.
 * WebKit can leave a fetch hanging across an iOS suspension. A shared read
 * would then hold every later re-apply behind it, which the host's
 * `loadPreferences` guard also rules out. So a response older than one already
 * applied is dropped.
 */
function applyDevicePreferences(prefs: Record<string, string>, attempt: number): void {
  if (attempt < devicePreferencesApplied) return;
  devicePreferencesApplied = attempt;
  devicePreferencesLoaded = true;
  cacheExternalLinkTarget(prefs['external_link_target']);
  applyAutocorrectPreference(prefs);
  // Motion and theme effects are appearance, which a shell push owns.
  if (hostAppearance) return;
  applyMotionPreference(prefs);
  applyThemeEffectsPreference(prefs);
}

/** Put the device's resolved motion on `<html>` as `data-motion`. It is the
 *  attribute `sdk-prefs.js` sets at first paint, so an app keys its own
 *  animations on one selector. */
function applyMotionPreference(prefs: Record<string, string>): void {
  lastMotionPreference = parseMotion(prefs[PREF_MOTION.key] || wsLocalGet(MOTION_STORAGE_KEY));
  resolveMotionAttribute();
}

/** Re-resolve `data-motion` from the kept preference and the live OS switch.
 *  Needs no fetch, so an OS flip costs nothing. */
function resolveMotionAttribute(): void {
  if (typeof document === 'undefined' || lastMotionPreference === null) return;
  const osReduces = window.matchMedia?.(REDUCED_MOTION_QUERY).matches === true;
  document.documentElement.setAttribute(
    'data-motion', motionAttribute(resolveReducedMotion(lastMotionPreference, osReduces)),
  );
}

/** Put the device's resolved theme effects on `<html>` as `data-theme-effects`,
 *  as `sdk-prefs.js` does at first paint. `sdk-iframe.css` then drops part
 *  shadows and filters on `reduce`, and an app may key its own glows on it. */
function applyThemeEffectsPreference(prefs: Record<string, string>): void {
  lastThemeEffectsPreference = parseThemeEffects(
    prefs['theme-effects'] || wsLocalGet(THEME_EFFECTS_STORAGE_KEY),
  );
  resolveThemeEffectsAttribute();
}

/** Re-resolve `data-theme-effects` from the kept preference and the live OS
 *  signals. Needs no fetch. */
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

/** Read this device's preferences once at load, without `applyPreferences`.
 *
 *  Needed because `applyPreferences` is OPTIONAL. An app shipping its own
 *  complete visual identity never calls it, yet two SDK behaviors still read
 *  the device's preferences:
 *
 *  - The delegated link handler reads {@link externalLinkTargetCache}. With a
 *    null cache every link takes the host path, which ignores "Ask": the
 *    activation `navigator.share` needs is gone by then.
 *  - The autocorrect stamp (`autocorrectStamp.ts`) starts from the seed or the
 *    default of on, and this read corrects it to the device's stored switch.
 *
 *  Called at load from `browser.ts`, on every client, since the switch is read
 *  everywhere. A failure keeps what the SDK had: a null link cache, which takes
 *  the host path, and the stamp's load-time value.
 *
 *  Not live: an app that never calls `watchPreferences` keeps what it saw at
 *  load until the next reload. Subscribing SSE from every app iframe to catch a
 *  rare mid-session change is not worth the connection. */
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

/** Whether this frame is inside an installed iOS PWA. The app iframe inherits
 *  the host's display mode, so the same check the host makes works here. */
function inIOSStandalone(): boolean {
  if (typeof navigator === 'undefined' || typeof window === 'undefined') return false;
  if (!isIOSAgent()) return false;
  return window.matchMedia?.('(display-mode: standalone)').matches === true
    || (navigator as Navigator & { standalone?: boolean }).standalone === true;
}

const HTTP_SCHEME_RE = /^https?:\/\//i;

/** Names this module currently has written. A name dropped from the map is
 *  then cleared on a live re-apply, rather than stuck at its last value. */
let appliedOverrideNames: string[] = [];

/**
 * The live style remote, iframe realm.
 *
 * The validator and its caps come from the shared appearance contract, which
 * the boot script and the host's `utils/styleOverrides.ts` read too. Any app
 * and the chat agent can write the preference. It is therefore an untrusted
 * path into inline style, and a rule relaxed in one realm would be a hole in
 * all of them. One copy is what stops that.
 *
 * A corrupt map parses to empty, which costs the app no theme and also clears
 * anything a previous apply had set. A cleared name falls back to `beneath`,
 * the theme's value, so dropping an override never drops the theme with it.
 */
function applyStyleOverrides(raw: string | null | undefined, beneath: Record<string, string>): void {
  appliedOverrideNames = replaceInlineTokens(
    document.documentElement.style, appliedOverrideNames, parseStyleOverrides(raw), beneath,
  );
}

/** The active theme. Seeded from what the boot script painted, so an app that
 *  never reaches the engine keeps its first-paint theme and font. */
let activeTheme: ResolvedTheme = parseResolvedTheme(
  servedPrefs()?.[THEME_SEED_KEY] ?? wsLocalGet(THEME_STORAGE_KEY),
);
/** The names this module has written. Seeded with every name the boot script
 *  may have set, so a switch away from a theme removes all of them. */
let appliedThemeNames: string[] = themeTokenNames(activeTheme);
/** The theme id `activeTheme` holds. Null until a fetch lands, so the first apply
 *  fetches, and so does every apply after a failed one. */
let themeId: string | null = null;
/** Set when the active theme's file or plugin changed, so the next apply
 *  fetches again although the id is the same. */
let themeStale = false;

/** The workspace fonts this frame holds entries for, besides the theme's own.
 *  Seeded with the one the engine resolved for first paint. */
const seededWorkspaceFont = parseWorkspaceFont(servedPrefs()?.[WORKSPACE_FONT_SEED_KEY]);
let workspaceFonts: WorkspaceFont[] = seededWorkspaceFont ? [seededWorkspaceFont] : [];
/** Set when the list may have changed, so the next apply that needs a
 *  workspace font fetches it. A frame with no seed has never seen the list. */
let workspaceFontsStale = seededWorkspaceFont === null;

/** The workspace fonts `GET /api/v1/fonts` lists, or null on a failure, which
 *  keeps the entries already held. */
async function fetchWorkspaceFonts(): Promise<WorkspaceFont[] | null> {
  try {
    const listing = await request<{ fonts?: Array<{ source?: unknown }> }>('/fonts');
    return sanitizeWorkspaceFonts((listing?.fonts ?? []).filter(f => f?.source === 'workspace'));
  } catch (err) {
    console.warn('[lucidos-sdk] could not load the workspace fonts:', err);
    return null;
  }
}

/** Fetch the theme a device's preferences name. A theme that no longer exists is
 *  the default theme. Any other failure answers null, which keeps the theme
 *  already painted until the next apply retries. */
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

/**
 * Paint the appearance `prefs` names, against the active theme and the
 * workspace fonts already in hand. Synchronous, so a shell push repaints in
 * the task it arrives in.
 *
 * Every value falls back to the mirror sdk-prefs.js read, so a value missing
 * from `prefs` keeps what first paint showed rather than resetting it.
 */
function paint(prefs: Record<string, string>): void {
  // The preference is kept, not just its resolution: `watchPreferences` has to
  // know whether this frame follows the OS before it acts on an OS flip.
  lastThemeModePreference = resolveThemeModePreference(
    prefs[THEME_MODE_KEY],
    wsLocalGet(THEME_MODE_STORAGE_KEY),
    () => document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE),
  );
  const mode = resolveThemeMode(lastThemeModePreference, osPrefersLight());
  const themeTokens = activeTheme[mode];
  const root = document.documentElement;
  // Stale names go first, so the theme's own inline background below is
  // never removed.
  for (const name of appliedThemeNames) {
    if (!(name in themeTokens)) root.style.removeProperty(name);
  }
  const bg = themeBackground(themeTokens) ?? THEME_MODE_BG[mode];
  root.setAttribute(THEME_MODE_ATTRIBUTE, mode);
  root.style.setProperty('--bg-primary', bg);
  // Mirrors sdk-prefs.js: keeps <html> covered before/after the iframe's
  // stylesheet applies its bg rule (iOS WKWebView underlying white).
  root.style.background = bg;
  appliedThemeNames = replaceInlineTokens(root.style, [], themeTokens);
  // A background that is not a hex literal reaches the canvas through the var.
  if (themeTokens['--bg-primary'] && !themeBackground(themeTokens)) root.style.background = 'var(--bg-primary)';

  // Font: the user's pick, else the theme's, else the fallback.
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

  // The live style remote's custom property overrides. LAST, because the
  // theme apply above writes --bg-primary, which the remote may override, and
  // inline properties are last-write-wins.
  applyStyleOverrides(prefs['style_overrides'] || wsLocalGet(STYLE_OVERRIDES_STORAGE_KEY), themeTokens);
}

function storedFontPreference(prefs: Record<string, string>): string | null {
  return prefs[PREF_FONT_FAMILY.key] || wsLocalGet(FONT_FAMILY_STORAGE_KEY);
}

/** Take the shell's push and repaint from it, with no request. The shell
 *  resolved the theme and the picked workspace font, so the push carries both. */
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
  /** Fetch user preferences and apply the theme mode, theme, font and scale as CSS
   *  variables. Once a shell push owns the appearance, apply only the
   *  external-link target and Autocorrect. */
  async applyPreferences(): Promise<void> {
    const attempt = ++devicePreferencesIssued;
    const prefs = await prefsModule.get();
    // Each `!hostAppearance` is checked again after an await: a push landing
    // during a fetch is newer than its answer.
    if (!hostAppearance) {
      // The theme is fetched only when it changed, so a mode flip or an
      // unrelated preference costs no request.
      const nextThemeId = prefs[THEME_KEY] || PREF_THEME.fallback;
      if (nextThemeId !== themeId || themeStale) {
        const theme = await fetchTheme(nextThemeId);
        // A newer apply started while this one fetched, and its answer wins.
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

    // The external-link target, which openExternal reads WITHOUT awaiting, the
    // Autocorrect switch and, with no shell push, motion. A live re-apply is
    // what lets a watching app follow a flip of any of them.
    applyDevicePreferences(prefs, attempt);
  },

  watchPreferences(): void {
    if (watchingPrefs) return;
    watchingPrefs = true;
    // Best-effort live re-application. A transient prefs-fetch failure must not
    // surface as an unhandled rejection. The next PreferencesChanged re-runs
    // applyPreferences, as does an OS light/dark flip in a frame with no shell
    // push, and the app keeps its appearance meanwhile. Warn, so a persistent
    // failure is still visible to a developer.
    const reapply = () => {
      ui.applyPreferences().catch((err) => {
        console.warn('[lucidos-sdk] live preference re-apply failed:', err);
      });
    };
    // The shell repaints this frame with itself, on every paint it makes. That
    // covers a change on another device and an edited theme file too, since
    // the shell follows both. Only a frame inside a shell ever hears this.
    onHostPush(APPEARANCE_CHANNEL, adoptHostAppearance);
    sse.on('PreferencesChanged', (data: unknown) => {
      // A `theme` write refetches even when it names the theme already painted:
      // that is how an agent republishes a theme it edited in place.
      const key = (data as { key?: unknown } | null)?.key;
      if (key === THEME_KEY) themeStale = true;
      if (key === PREF_FONT_FAMILY.key) workspaceFontsStale = true;
      reapply();
    });
    // Editing the active theme's file, or installing or removing the plugin
    // that ships it, repaints every frame showing it.
    const refetchTheme = () => {
      themeStale = true;
      // The shell refetches the theme too, and pushes the result.
      if (!hostAppearance) reapply();
    };
    // A workspace font changing can change the active theme too, since the
    // theme resolves the fonts it names.
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

    // An OS light/dark flip under a `system` preference emits no
    // PreferencesChanged, so the media query and the resume events drive it
    // instead. The follower and its guards are the host shell's own, and
    // `crates/lucidos-app/src/store/actions/preferences.ts` carries the full
    // reasoning.
    //
    // Sampling here rather than inside `applyPreferences` is what keeps a wake
    // that changed nothing free: `applyPreferences` fetches. A pushed frame
    // already holds everything it paints, so it repaints with no fetch at all.
    const { schedule: scheduleRefresh } = followSystemThemeMode({
      visible: () => document.visibilityState === 'visible',
      moved: () => currentThemeModePreference() === 'system'
        && document.documentElement.getAttribute(THEME_MODE_ATTRIBUTE) !== resolveThemeMode('system', osPrefersLight()),
      apply: () => {
        if (hostAppearance) paint(hostAppearance);
        else reapply();
      },
    });
    // Held in a variable rather than subscribed to inline. A `MediaQueryList`
    // with no strong reference has historically been collected in WebKit, which
    // takes its listener with it. `preferences.ts` keeps its own for the same
    // reason, and this is the engine where the theme has to keep working.
    systemThemeModeQuery = window.matchMedia('(prefers-color-scheme: light)');
    systemThemeModeQuery.addEventListener('change', scheduleRefresh);
    document.addEventListener('visibilitychange', scheduleRefresh);
    window.addEventListener('focus', scheduleRefresh);
    window.addEventListener('pageshow', scheduleRefresh);
    // An OS reduce-motion flip under a `system` motion preference emits no
    // PreferencesChanged either, so the media query drives it.
    reducedMotionQuery = window.matchMedia(REDUCED_MOTION_QUERY);
    reducedMotionQuery.addEventListener('change', resolveMotionAttribute);
    // The same holds for the two signals `system` theme effects follow.
    themeEffectsQueries = [REDUCED_TRANSPARENCY_QUERY, MORE_CONTRAST_QUERY].map(q => window.matchMedia(q));
    for (const query of themeEffectsQueries) query.addEventListener('change', resolveThemeEffectsAttribute);
  },

  /**
   * Request navigation in the Lucidos frontend.
   * Calls POST /api/v1/ui/navigate, which emits a NavigationRequested event
   * that the frontend subscribes to via SSE.
   */
  navigate(target: NavigateTarget, params: NavigateParams = {}): Promise<void> {
    assertPlainObject('params', params);
    return requestVoid('/ui/navigate', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ target, params }),
    });
  },

  /**
   * Open a URL outside Lucidos, honouring the user's external-link preference.
   *
   * Use this instead of `window.open` for any link out of your app. From inside
   * an app iframe `window.open` cannot escape an installed iOS PWA: WebKit
   * renders it in the PWA's in-app web view, which has no address bar, no tabs
   * and no shared Safari session. Anchors are already handled for you (the SDK
   * delegates `<a href="https://…">` clicks automatically); this is the
   * programmatic equivalent for links opened from JS.
   *
   * MUST be called synchronously from the user's click/tap handler. In the
   * user's "Ask" mode this opens the OS share sheet via `navigator.share`, which
   * the browser refuses without transient user activation, and any `await`
   * before this call spends that activation. Do the async work first, then call
   * this from a later gesture.
   *
   * Non-http(s) URLs (`mailto:`, `tel:`) are handed to the platform unchanged.
   * Resolves once the open has been dispatched; a user dismissing the share
   * sheet is a normal resolve, not a rejection.
   */
  openExternal(url: string): Promise<void> {
    assertString('url', url);
    // Share only where the preference can apply and only for a web page. Off
    // iOS, and for `mailto:` / `tel:`, the host path is always right.
    if (
      inIOSStandalone()
      && HTTP_SCHEME_RE.test(url)
      && externalLinkTargetCache === 'ask'
      && typeof navigator.share === 'function'
    ) {
      // Deliberately NOT routed through the host: user activation does not
      // survive the hop (the host's navigate goes over HTTP and lands via SSE),
      // so a host-side share would be refused on every app link.
      //
      // In an app frame this no longer reaches the sheet. ADR 0227 gave the
      // frame an opaque origin, which matches no feature's `self` default, and
      // `web-share` is not delegated. WebKit refuses it even when it is, so
      // the attribute is no remedy and iOS is the only caller. The catch below
      // takes the host path, so the link still opens. A standalone app tab is
      // a top-level document and still gets the sheet.
      return navigator.share({ url }).catch((err: unknown) => {
        // The user closing the sheet chose "none of these"; honour it rather
        // than opening something anyway. Anything else means the sheet never
        // worked, so fall through to the host.
        if (err instanceof Error && err.name === 'AbortError') return;
        return ui.navigate('url', { url });
      });
    }
    return ui.navigate('url', { url });
  },

  /**
   * Open a fresh chat thread, optionally prefilling the compose textarea.
   * The user must click Send: this never auto-submits. On an app, trigger or
   * settings panel it closes the overlay first. With a thread focused it drops
   * the focus, so the compose lands on a new thread.
   */
  startThread(opts?: { prompt?: string }): Promise<void> {
    const params: NavigateParams = {};
    if (opts && opts.prompt !== undefined) {
      if (typeof opts.prompt !== 'string') {
        return Promise.reject(new TypeError('opts.prompt must be a string'));
      }
      if (opts.prompt.length > 0) params.prompt = opts.prompt;
    }
    return ui.navigate('new-chat', params);
  },

  /**
   * Show a confirmation dialog rendered by the host shell (above all app
   * content, themed by the user's preferences). Resolves to `true` on OK
   * and `false` on Cancel / Esc / backdrop click. If another confirm is
   * already showing, this one replaces it (the previous resolves `false`).
   */
  confirm(options: ConfirmOptions): Promise<boolean> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    // No parent, e.g. the SDK loaded in a top-level window. Fall back to native
    // window.confirm so the API still works in standalone testing contexts.
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
      // No deadline, unlike `previewFile`. The host answers when the reader
      // decides, however long that takes. A deadline answered Cancel under a
      // dialog still open, and dropped the press that followed.
      pendingConfirms.set(id, resolve);
      window.parent.postMessage({ type: 'lucidos:ui:confirm', id, payload }, '*');
    });
  },

  /**
   * Show a transient toast rendered by the host shell (above all app content,
   * themed by the user's preferences). Fire-and-forget, with no result.
   *
   * Only the serializable subset of the host toast is exposed: `message`,
   * the `type` severity, and `opts` (an optional `title` among them). The host's action-button callbacks cannot
   * cross the postMessage boundary, so they are deliberately unavailable here.
   * An unknown `type` degrades to `info`.
   */
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
    // No host parent, so surface via console: a standalone testing context then
    // still sees the feedback instead of silence.
    if (window.parent === window) {
      const line = `[lucidos.ui.toast:${safeType}] ${payload.title ? `${payload.title}: ` : ''}${message}`;
      if (safeType === 'error') console.error(line);
      else if (safeType === 'warning') console.warn(line);
      else console.log(line);
      return;
    }
    window.parent.postMessage({ type: 'lucidos:ui:toast', payload }, '*');
  },

  /**
   * Take down a toast your app raised with `toast(…, { key })`. Fire-and-forget,
   * like `toast` itself. Use it for the case a keyed replacement can't express:
   * work that finishes with nothing left to say, e.g. a `spinning` "Syncing…"
   * toast that should just disappear when the SSE event lands.
   *
   * A key matching nothing is a silent no-op. Your app cannot know whether the
   * toast is still up (the user may have closed it, or its duration may have
   * expired), so "already gone" is the normal case, not an error.
   */
  dismissToast(key: string): void {
    if (typeof key !== 'string' || key.length === 0) {
      throw new TypeError('lucidos.ui.dismissToast: key must be a non-empty string');
    }
    // No host parent, so mirror the console fallback in `toast()`. A standalone
    // testing context then sees both halves of the exchange, instead of a toast
    // line with no matching dismissal.
    if (window.parent === window) {
      console.log(`[lucidos.ui.dismissToast] ${key}`);
      return;
    }
    window.parent.postMessage({ type: 'lucidos:ui:dismissToast', payload: { key } }, '*');
  },

  /**
   * Tell the host your content is on screen, so it can lift its loading cover.
   * Only an app whose `manifest.json` declares `"reveal": "on-ready"` is held
   * for this; any other app is revealed on its page `load`, and the call does
   * nothing. Call it once your first data has rendered. A repeated call is
   * harmless, and in its own tab the app has no host, so nothing is sent.
   */
  ready(): void {
    if (window.parent === window) return;
    window.parent.postMessage({ type: 'lucidos:ui:ready' }, '*');
  },

  /**
   * Prompt for a line of text via a modal rendered by the host shell (above all
   * app content, themed by the user's preferences). Resolves to the entered
   * string on OK/Enter, or `null` on Cancel / Esc / backdrop click. If another
   * prompt is already showing, this one replaces it (the previous resolves
   * `null`). Use it instead of `window.prompt()`.
   */
  prompt(options: PromptOptions): Promise<string | null> {
    assertPlainObject('options', options);
    if (typeof options.message !== 'string' || options.message.length === 0) {
      return Promise.reject(new TypeError('options.message must be a non-empty string'));
    }
    // No parent, e.g. the SDK loaded in a top-level window. Fall back to native
    // window.prompt so the API still works in standalone testing contexts.
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
      // No deadline, as for `confirm`: the reader may type for minutes, and a
      // deadline discarded what they typed.
      pendingPrompts.set(id, resolve);
      window.parent.postMessage({ type: 'lucidos:ui:prompt', id, payload }, '*');
    });
  },

  /**
   * Show a file in a read-only modal rendered by the host, over your app,
   * WITHOUT navigating away. Use it for a citation in a report or a dashboard:
   * the reader glances at the code and carries on, instead of losing their place
   * in the Files panel and having to navigate back. The modal carries a link
   * that escalates into the full Files preview when they do want to leave.
   *
   * Takes the same locators as `navigate('file', …)` (a workspace data path, a
   * `repo:<repoId>:file:<path>` one, or `repo:<repoId>:file#<ref>:<path>` to
   * name a branch, tag or sha) and the same `line` / `line_end`, with the same
   * degradation: a line the file cannot honour (`0`, negative, fractional, past
   * the end, a format with no source view) opens the file at the top rather
   * than refusing it.
   *
   * Resolves once the preview is on screen, NOT when the reader dismisses it: a
   * glance can stay open for minutes, and your app is not blocked while it is.
   * Rejects when the host cannot show it, which makes the escalation a natural
   * fallback:
   *
   * ```js
   * try { await lucidos.ui.previewFile(at); }
   * catch { await lucidos.ui.navigate('file', at); }
   * ```
   *
   * Two things make it reject: your app is running with no host shell around it
   * (opened in its own tab, or the SDK loaded in a top-level page), and a
   * fullscreen element the host cannot render over. Both mean the same thing,
   * that nothing would appear, and the fallback above is what turns that into
   * something the reader can act on.
   *
   * A second call replaces a showing preview. Read-only: there is no editing in
   * the modal.
   */
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
    // No host shell around this window, so there is no modal to show and no
    // reply to wait for. Reject rather than quietly calling `navigate` here:
    // that request goes through the engine and lands in whichever OTHER window
    // runs the shell. The reader clicking a citation would see nothing happen.
    // A different window would navigate its Files panel, and this promise would
    // resolve as if it had worked. The escalation is the app author's to make,
    // from its own catch.
    if (window.parent === window) {
      return Promise.reject(new Error(
        'lucidos.ui.previewFile: no host to show the preview (this app is not running inside Lucidos)',
      ));
    }
    installPreviewListener();
    const id = `v${++previewCounter}-${Date.now()}`;
    return new Promise<void>((resolve, reject) => {
      // Bounds a LOST reply, not the reader's time: the host answers as soon as
      // it has decided, so this only fires when nothing answered at all. Without
      // it a host crash would leak the Map entry forever.
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
      window.parent.postMessage({ type: 'lucidos:ui:preview-file', id, payload }, '*');
    });
  },

  /** Themed dropdown, replacing native `<select>` so popups can be styled. */
  Select,

  /**
   * Enhance every `<select class="lucidos-select">` under `root` (default
   * `document`) with a themed dropdown. The native element stays in the DOM,
   * hidden, so existing form code keeps working: `change` fires on it and its
   * `value` mirrors the user's selection.
   */
  enhanceSelects,

  /**
   * Turn the built-in tooltip off, for an app that ships its own. The SDK
   * installs the tooltip on load, so this is an override rather than a switch:
   * call it once at startup. `data-lucidos-tooltips="off"` on `<html>` or
   * `<body>` does the same from markup, before any script runs.
   *
   * Neither is needed just because the app renders its own `#tooltip`: the
   * layer already stands down when it finds one.
   */
  disableTooltips,
};
