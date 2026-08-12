/* GENERATED from packages/lucidos-sdk/src/boot/ by appearanceBoot.build.mjs.
   Do not edit: run `npm run build` in packages/lucidos-sdk. */
"use strict";
(() => {
  // src/appearance.ts
  var THEMES = ["light", "dark", "system"];
  var DEFAULT_THEME = "system";
  var THEME_BG = {
    light: "#ffffff",
    dark: "#07172e"
  };
  var FONT_FAMILY_VALUES = {
    monospace: "ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'Fira Code', 'JetBrains Mono', Monaco, Consolas, monospace",
    system: "system-ui, -apple-system, BlinkMacSystemFont, 'Segoe UI', sans-serif",
    inter: "'Inter', system-ui, sans-serif",
    "jetbrains-mono": "'JetBrains Mono', monospace",
    "ibm-plex-mono": "'IBM Plex Mono', monospace",
    "fira-code": "'Fira Code', ui-monospace, SFMono-Regular, 'SF Mono', Menlo, 'JetBrains Mono', Monaco, Consolas, monospace"
  };
  var DEFAULT_FONT_FAMILY = "fira-code";
  var FONT_FEATURES_DEFAULT = { text: "normal", code: "normal" };
  var FONT_FEATURES = {
    "fira-code": { text: '"liga" 0, "calt" 0', code: '"liga" 1, "calt" 1' }
  };
  var UI_SCALE_MIN = 75;
  var UI_SCALE_MAX = 200;
  var UI_SCALE_STEP = 12.5;
  var LEGACY_UI_SCALES = {
    small: 100,
    medium: 112.5,
    large: 125
  };
  function resolveTheme(theme, prefersLight) {
    if (theme === "system") return prefersLight ? "light" : "dark";
    return theme;
  }
  function resolveFontKey(stored) {
    return stored && hasOwn(FONT_FAMILY_VALUES, stored) ? stored : DEFAULT_FONT_FAMILY;
  }
  function fontFeaturesFor(font) {
    return hasOwn(FONT_FEATURES, font) ? FONT_FEATURES[font] : FONT_FEATURES_DEFAULT;
  }
  function hasOwn(obj, key) {
    return Object.prototype.hasOwnProperty.call(obj, key);
  }
  function clampUiScale(scale) {
    const snapped = Math.round(scale / UI_SCALE_STEP) * UI_SCALE_STEP;
    return Math.max(UI_SCALE_MIN, Math.min(UI_SCALE_MAX, snapped));
  }
  function parseUiScale(raw) {
    if (!raw) return null;
    const n = hasOwn(LEGACY_UI_SCALES, raw) ? LEGACY_UI_SCALES[raw] : parseFloat(raw);
    if (isNaN(n)) return null;
    return clampUiScale(n);
  }
  var STYLE_OVERRIDES_STORAGE_KEY = "lucidos-style-overrides";
  var STYLE_RESET_PARAM = "style-reset";
  var MAX_STYLE_OVERRIDES = 200;
  var MAX_STYLE_VALUE_LENGTH = 120;
  var NAME_RE = /^--[a-z][a-z0-9-]*$/;
  var VALUE_BANNED_RE = /[;{}<>@\\]|url\s*\(|image-set\s*\(|expression\s*\(|\/\*/i;
  function isValidOverrideName(name) {
    return NAME_RE.test(name);
  }
  function isValidOverrideValue(value) {
    if (typeof value !== "string") return false;
    const trimmed = value.trim();
    if (trimmed === "") return false;
    if (trimmed.length > MAX_STYLE_VALUE_LENGTH) return false;
    return !VALUE_BANNED_RE.test(trimmed);
  }
  function parseStyleOverrides(raw) {
    if (!raw) return {};
    let parsed;
    try {
      parsed = JSON.parse(raw);
    } catch (e) {
      return {};
    }
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return {};
    const out = {};
    let n = 0;
    for (const [name, value] of Object.entries(parsed)) {
      if (n >= MAX_STYLE_OVERRIDES) break;
      if (!isValidOverrideName(name)) continue;
      if (typeof value !== "string" || !isValidOverrideValue(value)) continue;
      out[name] = value.trim();
      n++;
    }
    return out;
  }
  function styleResetRequested(search) {
    return new RegExp(`[?&]${STYLE_RESET_PARAM}(?:[=&]|$)`).test(search);
  }

  // src/_fetch.ts
  function computeBaseUrl() {
    var _a;
    if (typeof document !== "undefined") {
      const href = (_a = document.querySelector("base")) == null ? void 0 : _a.getAttribute("href");
      if (href) {
        let path2 = href;
        try {
          if (/^https?:\/\//i.test(href)) path2 = new URL(href).pathname;
        } catch (e) {
        }
        return path2.replace(/\/+$/, "");
      }
    }
    const path = typeof window !== "undefined" && window.location && window.location.pathname || "";
    const i = path.indexOf("/app/");
    return i >= 0 ? path.slice(0, i) : "";
  }
  var _baseUrl = computeBaseUrl();
  function getBaseUrl() {
    return _baseUrl;
  }

  // src/_storage.ts
  function workspaceSlug() {
    const base = getBaseUrl();
    if (!base) return null;
    const seg = base.replace(/^\/+|\/+$/g, "");
    return seg === "" || seg === "~" ? null : seg;
  }
  function nsKey(key) {
    const slug = workspaceSlug();
    return slug ? `ws:${slug}:${key}` : key;
  }
  function wsLocalGet(key) {
    try {
      return localStorage.getItem(nsKey(key));
    } catch (e) {
      return null;
    }
  }
  function wsLocalRemove(key) {
    try {
      localStorage.removeItem(nsKey(key));
    } catch (e) {
    }
  }

  // src/boot/appearanceBoot.ts
  function applyAppearanceBoot(opts) {
    const d = document.documentElement;
    const raw = wsLocalGet("lucidos-theme");
    const theme = raw && THEMES.includes(raw) ? raw : DEFAULT_THEME;
    const prefersLight = matchMedia("(prefers-color-scheme: light)").matches;
    const resolved = resolveTheme(theme, prefersLight);
    d.setAttribute("data-theme", resolved);
    const bg = THEME_BG[resolved];
    d.style.setProperty("--bg-primary", bg);
    d.style.background = bg;
    const fontKey = resolveFontKey(wsLocalGet("lucidos-font-family"));
    d.style.setProperty("--font-ui", FONT_FAMILY_VALUES[fontKey]);
    const features = fontFeaturesFor(fontKey);
    d.style.setProperty("--font-features-text", features.text);
    d.style.setProperty("--font-features-code", features.code);
    const scale = parseUiScale(wsLocalGet("lucidos-ui-scale"));
    if (scale !== null) d.style.setProperty("--user-ui-scale", `${scale}%`);
    try {
      if (opts.styleReset && styleResetRequested(location.search)) {
        wsLocalRemove(STYLE_OVERRIDES_STORAGE_KEY);
      } else {
        const overrides = parseStyleOverrides(wsLocalGet(STYLE_OVERRIDES_STORAGE_KEY));
        for (const name of Object.keys(overrides)) {
          d.style.setProperty(name, overrides[name]);
        }
      }
    } catch (e) {
    }
    return { raw, theme, resolved, prefersLight };
  }

  // src/boot/host.ts
  var SPLASH_BACKGROUND = "#145eb9 radial-gradient(125% 125% at 30% 22%, #2d83e0 0%, #0a4ea8 100%) no-repeat fixed";
  var boot = applyAppearanceBoot({ styleReset: true });
  document.documentElement.style.background = SPLASH_BACKGROUND;
  try {
    const t0 = performance.now();
    window.__themeLogEvt = (label, info) => {
      fetch("api/v1/internal/client-log", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({
          category: "theme",
          message: label,
          data: Object.assign({ tMs: Math.round(performance.now() - t0) }, info)
        }),
        keepalive: true
      }).catch(() => {
      });
    };
    window.__themeLogEvt("fouc", {
      raw: boot.raw,
      theme: boot.theme,
      resolved: boot.resolved,
      mqLight: boot.prefersLight
    });
    window.addEventListener("pageshow", (e) => {
      var _a;
      (_a = window.__themeLogEvt) == null ? void 0 : _a.call(window, "pageshow", {
        persisted: e.persisted,
        dataTheme: document.documentElement.getAttribute("data-theme"),
        rawNow: wsLocalGet("lucidos-theme"),
        mqLightNow: matchMedia("(prefers-color-scheme: light)").matches
      });
    });
  } catch (e) {
  }
})();
