// The HTML artifact preview frame, and the message bridge that crosses it.
//
// A previewed artifact is untrusted: an upload, an agent's web or email fetch,
// or a file an app wrote. So its frame runs at an OPAQUE origin (ADR 0322).
// Its scripts still run, but it cannot read the shell's DOM or storage, and
// the engine refuses its requests to `/api/v1` as cross-site.
//
// The host cannot reach into an opaque frame's `contentDocument`. So a small
// script is stamped into the document, ahead of the artifact's own. It carries
// link clicks, downloads, shell shortcuts and fragment scrolls. It cancels what
// the browser must not do, and posts the rest up. Every routing DECISION stays
// on the host.
//
// The UI-scale zoom needs no bridge: `withPreviewSizing` stamps it into the
// document text. The zoom SHORTCUTS ride the chord bridge like every other.

import type { Binding } from '../../utils/shortcuts';
import { isMac } from '../../utils/platform';
import { lucidos } from '@lucidos/sdk';
import { dispatchForwardedChord } from '../../hooks/useKeyboardShortcuts';
import { showToast } from '../../store/store';
import { scrollBehavior } from '../../utils/motion';
import {
  PREVIEW_FRAME_MESSAGE,
  PREVIEW_HOST_MESSAGE,
  postToPreviewFrame,
} from '../../utils/previewFrameProtocol';
import {
  PREVIEW_HOST_SCHEMES,
  type PreviewLinkAction,
  classifyPreviewLink,
  previewLinkContext,
  resolvePreviewRelativePath,
  runPreviewLinkAction,
} from './previewIframeLinks';

/**
 * What a preview frame may do.
 *
 * `allow-same-origin` is absent, and that is the fix: with it the artifact
 * would run as the shell. `allow-top-navigation` is absent, so it cannot move
 * the shell. `allow-popups-to-escape-sandbox` is absent, so a window it opens
 * stays sandboxed. The engine serves a data file under the same set
 * (`file_response::DOCUMENT_SANDBOX_CSP`).
 */
export const ARTIFACT_PREVIEW_SANDBOX =
  'allow-scripts allow-forms allow-modals allow-popups allow-downloads';

/**
 * What the shell delegates to a preview frame, as a permissions policy.
 *
 * The opaque origin is not `self`, so every feature is denied unless listed.
 * `autoplay` and `fullscreen` keep a report's media playing and its video's
 * fullscreen button working. `clipboard-write` keeps its Copy buttons working.
 * The same calls the app frame makes (`appFrameSandbox.ts`), minus
 * `encrypted-media`, which no report needs.
 */
export const ARTIFACT_PREVIEW_ALLOW = 'autoplay; fullscreen; clipboard-write';

/** The path segment that carries a frame capability. Mirrors
 *  `lucidos-frame-capability::SEGMENT`. */
const CAPABILITY_SEGMENT = '~cap';

/**
 * Splice the asset pass into an artifact's URL on the `/data` mount, ahead of
 * its `/data/artifacts/` segment.
 *
 * The preview's `<base href>` is built from the result. So every relative ref
 * in the artifact carries the pass to the gateway, which the frame's cookieless
 * requests need (ADR 0238). `null` is no gateway, where nothing needs proving.
 * A URL off the mount comes back untouched: the pass reaches no `/api/v1`.
 */
export function withPreviewCapability(fileUrl: string, capability: string | null): string {
  if (!capability) return fileUrl;
  const at = fileUrl.indexOf('/data/artifacts/');
  if (at === -1 || fileUrl.slice(0, at).endsWith('/api/v1')) return fileUrl;
  return `${fileUrl.slice(0, at)}/${CAPABILITY_SEGMENT}/${capability}${fileUrl.slice(at)}`;
}

/**
 * A fresh per-render nonce. The host drops a message that does not carry it.
 *
 * `getRandomValues` rather than `randomUUID`: the second exists only in a
 * secure context, and a phone may reach a dev workspace over plain http.
 */
export function newBridgeNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

/** What the host stamps into the bridge script. */
export interface PreviewBridgeConfig {
  nonce: string;
  bindings: readonly Binding[];
  hostSchemes: readonly string[];
}

/**
 * The script that runs inside the preview frame, as source. Plain ES5 over its
 * three arguments, with no outer name, so a test can run it against a fake window.
 *
 * What it cancels, it decides synchronously, from two stamped lists: the schemes
 * the host routes, and the current shortcut bindings. The click rule mirrors
 * `handlePreviewLinkClick`, and the chord matcher mirrors `matchesEvent`. A
 * sibling `download` is claimed too: across origins the browser would ignore
 * `download` and navigate the frame. Listeners sit on `document` in the capture
 * phase, after the artifact's own window-capture handlers.
 *
 * The nonce is what the host trusts, so the artifact must never learn it:
 *  - the script runs first, then removes its own element, so the nonce lives
 *    only in this closure;
 *  - each message is an object literal, which no prototype setter intercepts;
 *  - it posts only for a trusted event, never for a scripted `click()`.
 */
export const PREVIEW_BRIDGE_SOURCE = String.raw`function (cfg, win, doc) {
  var self = doc.currentScript;
  if (self && self.parentNode) self.parentNode.removeChild(self);
  var host = win.parent;
  if (!host || host === win) return;
  var nonce = cfg.nonce;
  var frameType = cfg.frameType;
  function post(msg) {
    host.postMessage(msg, '*');
  }
  function hostOwns(href) {
    if (!href) return false;
    var scheme = /^([a-z][a-z0-9+.-]*):/i.exec(href);
    return !scheme || cfg.schemes.indexOf(scheme[1].toLowerCase()) !== -1;
  }
  doc.addEventListener('click', function (e) {
    if (!e.isTrusted || e.defaultPrevented) return;
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || (e.button || 0) !== 0) return;
    var target = e.target;
    var anchor = target && target.closest ? target.closest('a[href]') : null;
    if (!anchor) return;
    var href = (anchor.getAttribute('href') || '').trim();
    if (anchor.hasAttribute('download')) {
      if (!href || /^[a-z][a-z0-9+.-]*:/i.test(href)) return;
      e.preventDefault();
      e.stopPropagation();
      post({ type: frameType, nonce: nonce, kind: 'download', href: href,
        name: anchor.getAttribute('download') || '', baseUri: doc.baseURI });
      return;
    }
    if (!hostOwns(href)) return;
    e.preventDefault();
    e.stopPropagation();
    post({ type: frameType, nonce: nonce, kind: 'link', href: href, baseUri: doc.baseURI });
  }, true);
  function normalizeKey(key) {
    if (key === '+') return '=';
    return key.length === 1 ? key.toLowerCase() : key;
  }
  function isShortcut(e) {
    var key = normalizeKey(e.key);
    for (var i = 0; i < cfg.bindings.length; i++) {
      var b = cfg.bindings[i];
      if ((e.metaKey || e.ctrlKey) === b.mod && e.shiftKey === b.shift
        && e.altKey === b.alt && key === b.key) return true;
    }
    return false;
  }
  function isEditable(t) {
    return !!t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName || ''));
  }
  function isMacTextEdit(e) {
    return cfg.mac && e.ctrlKey && !e.metaKey && !e.shiftKey && !e.altKey
      && /^[a-z]$/i.test(e.key) && isEditable(e.target);
  }
  doc.addEventListener('keydown', function (e) {
    if (!e.isTrusted || typeof e.key !== 'string') return;
    if (isMacTextEdit(e)) return;
    var shortcut = isShortcut(e);
    if (!shortcut && e.key !== 'Escape') return;
    if (shortcut) e.preventDefault();
    post({ type: frameType, nonce: nonce, kind: 'chord', key: e.key, metaKey: e.metaKey,
      ctrlKey: e.ctrlKey, shiftKey: e.shiftKey, altKey: e.altKey });
  }, true);
  function scrollToFragment(id, smooth) {
    var behavior = smooth ? 'smooth' : 'auto';
    if (!id) {
      win.scrollTo({ top: 0, behavior: behavior });
      return;
    }
    var quoted = id.replace(/["\\]/g, '\\$&');
    var target = doc.querySelector('[id="' + quoted + '"]')
      || doc.querySelector('[name="' + quoted + '"]');
    if (!target) {
      post({ type: frameType, nonce: nonce, kind: 'fragment-missing', id: id });
      return;
    }
    target.scrollIntoView({ behavior: behavior, block: 'start' });
  }
  function adoptCapability(capability) {
    if (!/^[0-9A-Za-z._~-]+$/.test(capability)) return;
    var base = doc.querySelector('base');
    var href = base && base.getAttribute('href');
    if (!href) return;
    var next = href.replace(/\/~cap\/[^\/]+\//, '/~cap/' + capability + '/');
    if (next !== href) base.setAttribute('href', next);
  }
  win.addEventListener('message', function (e) {
    if (e.source !== host) return;
    var data = e.data;
    if (!data || typeof data !== 'object' || data.type !== cfg.hostType) return;
    if (data.kind === 'scroll' && typeof data.id === 'string') {
      scrollToFragment(data.id, data.smooth === true);
    } else if (data.kind === 'capability' && typeof data.capability === 'string') {
      adoptCapability(data.capability);
    }
  });
}`;

/** The `<script>` tag that installs the bridge. JSON cannot close a script
 *  element once every `<` is escaped, so no artifact byte is needed. */
export function previewBridgeScript(cfg: PreviewBridgeConfig): string {
  const json = JSON.stringify({
    nonce: cfg.nonce,
    bindings: cfg.bindings,
    mac: isMac,
    schemes: cfg.hostSchemes,
    frameType: PREVIEW_FRAME_MESSAGE,
    hostType: PREVIEW_HOST_MESSAGE,
  }).replace(/</g, '\\u003c');
  return `<script>(${PREVIEW_BRIDGE_SOURCE})(${json}, window, document);</script>`;
}

/** Stamp the bridge in as the very first thing the document parses, after a
 *  doctype only. Not merely first in `<head>`. An artifact can put a script
 *  before its head, or a `<head>` inside a comment. Either would then run ahead
 *  of the bridge and watch it arrive. */
export function withPreviewBridge(html: string, cfg: PreviewBridgeConfig): string {
  // Comments may precede the doctype and still keep standards mode.
  const doctype = /^(?:\s|<!--[\s\S]*?-->)*<!doctype[^>]*>/i.exec(html);
  const at = doctype ? doctype[0].length : 0;
  return html.slice(0, at) + previewBridgeScript(cfg) + html.slice(at);
}

/** The bridge config for one render: a fresh nonce and today's lists. */
export function previewBridgeConfig(bindings: readonly Binding[]): PreviewBridgeConfig {
  return { nonce: newBridgeNonce(), bindings, hostSchemes: PREVIEW_HOST_SCHEMES };
}

/** A message from a preview frame, once it has passed every check. */
export type PreviewFrameMessage =
  | { kind: 'link'; href: string; baseUri: string | null }
  | { kind: 'download'; href: string; name: string; baseUri: string | null }
  | {
    kind: 'chord';
    chord: { key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean };
  }
  | { kind: 'fragment-missing'; id: string };

/** Longest href or anchor id the host will route. A real link is far shorter;
 *  the cap keeps a hostile frame from handing the router megabytes. */
const MAX_TEXT = 8192;

function boundedString(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_TEXT;
}

/**
 * Accept a message only when it provably came from THIS preview's document.
 *
 * Four checks, all required:
 *  - `source` is this frame's own window. Origin alone proves nothing: every
 *    opaque frame on the page, an app's included, reports `"null"`.
 *  - `origin` is `"null"`. The frame is sandboxed, so anything else means the
 *    sandbox is gone, and the bridge refuses to work rather than trust it.
 *  - `nonce` is the one stamped into this render. A message left over from the
 *    document the frame showed before is dropped.
 *  - the body has the exact shape of its kind.
 */
export function readPreviewFrameMessage(
  e: Pick<MessageEvent, 'source' | 'origin' | 'data'>,
  frameWindow: Window | null,
  nonce: string,
): PreviewFrameMessage | null {
  if (frameWindow === null || e.source !== frameWindow) return null;
  if (e.origin !== 'null') return null;
  const data = e.data as Record<string, unknown> | null;
  if (!data || typeof data !== 'object') return null;
  if (data.type !== PREVIEW_FRAME_MESSAGE || data.nonce !== nonce) return null;
  switch (data.kind) {
    case 'link':
      if (!boundedString(data.href)) return null;
      return { kind: 'link', href: data.href, baseUri: boundedString(data.baseUri) ? data.baseUri : null };
    case 'chord': {
      const flags = [data.metaKey, data.ctrlKey, data.shiftKey, data.altKey];
      if (typeof data.key !== 'string' || data.key.length > 32) return null;
      if (!flags.every((f) => typeof f === 'boolean')) return null;
      return {
        kind: 'chord',
        chord: {
          key: data.key,
          metaKey: data.metaKey as boolean,
          ctrlKey: data.ctrlKey as boolean,
          shiftKey: data.shiftKey as boolean,
          altKey: data.altKey as boolean,
        },
      };
    }
    case 'download':
      if (!boundedString(data.href) || !boundedString(data.name)) return null;
      return {
        kind: 'download',
        href: data.href,
        name: data.name,
        baseUri: boundedString(data.baseUri) ? data.baseUri : null,
      };
    case 'fragment-missing':
      if (!boundedString(data.id)) return null;
      return { kind: 'fragment-missing', id: data.id };
    default:
      return null;
  }
}

/** What the host knows about the preview a message came from. */
export interface PreviewFrameContext {
  artifactPath: string;
  /** Whether the artifact declares its own `<base href>`. Only then does the
   *  frame's reported base decide where a relative link points. */
  declaresOwnBase: boolean;
  frameWindow: Window | null;
}

/** Does the shell hold transient user activation right now?
 *
 *  A click inside the preview activates its ancestors too. So the shell reads
 *  true for a real click, and false for a message script sent on its own. A
 *  browser without the API answers `null`: unknown, not yes. */
function shellHasUserActivation(): boolean | null {
  const activation = (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation;
  return activation ? activation.isActive : null;
}

/** May a message from the preview carry out a navigation?
 *
 *  Only while the shell holds activation from a click. The bridge already posts
 *  only for a trusted event, under a nonce the artifact cannot read. This is
 *  the second lock: a browser that cannot tell answers `null` and passes. */
export function previewMayNavigate(activation: boolean | null): boolean {
  return activation !== false;
}

/** Act on a checked message from a preview frame. */
export function routePreviewFrameMessage(msg: PreviewFrameMessage, ctx: PreviewFrameContext): void {
  switch (msg.kind) {
    case 'link': {
      const documentBase = ctx.declaresOwnBase ? msg.baseUri ?? undefined : undefined;
      const action = classifyPreviewLink(msg.href, previewLinkContext(ctx.artifactPath, documentBase));
      if (!action) {
        showToast(`Can't open the link "${msg.href}" from the preview of ${ctx.artifactPath}`, 'error');
        return;
      }
      if (action.kind === 'fragment') {
        postToPreviewFrame(ctx.frameWindow, {
          kind: 'scroll',
          id: action.id,
          smooth: scrollBehavior() === 'smooth',
        });
        return;
      }
      if (!previewMayNavigate(shellHasUserActivation())) {
        refuseUnclicked(`open "${msg.href}"`, ctx.artifactPath);
        return;
      }
      runPreviewLinkAction(action, ctx.artifactPath);
      return;
    }
    case 'download': {
      if (!previewMayNavigate(shellHasUserActivation())) {
        refuseUnclicked(`download "${msg.href}"`, ctx.artifactPath);
        return;
      }
      const target = downloadTarget(msg, ctx);
      if (target.kind === 'file') downloadDataFile(target.path, msg.name);
      else runPreviewLinkAction(target, ctx.artifactPath);
      return;
    }
    case 'chord':
      dispatchForwardedChord(msg.chord);
      return;
    case 'fragment-missing':
      showToast(`No "${msg.id}" section in ${ctx.artifactPath}`, 'error');
      return;
  }
}

/** Where a download link points. A workspace file, normally. An artifact
 *  that declares its own `<base href>` may point it elsewhere, and then the
 *  link goes where that base says, as a link would. */
function downloadTarget(
  msg: Extract<PreviewFrameMessage, { kind: 'download' }>,
  ctx: PreviewFrameContext,
): Exclude<PreviewLinkAction, { kind: 'fragment' }> {
  const routed = ctx.declaresOwnBase && msg.baseUri
    ? classifyPreviewLink(msg.href, previewLinkContext(ctx.artifactPath, msg.baseUri))
    : null;
  if (routed?.kind === 'file') return { kind: 'file', path: decodeSegments(routed.path) };
  if (routed && routed.kind !== 'fragment') return routed;
  return { kind: 'file', path: decodeSegments(resolvePreviewRelativePath(ctx.artifactPath, msg.href)) };
}

/** An href is percent-encoded and `lucidos.data.url` encodes again, so a
 *  `Q3%20summary.csv` would be fetched as `Q3%2520summary.csv`. A malformed
 *  escape is kept as written. */
function decodeSegments(path: string): string {
  return path.split('/').map((segment) => {
    try {
      return decodeURIComponent(segment);
    } catch {
      return segment;
    }
  }).join('/');
}

function refuseUnclicked(what: string, artifactPath: string): void {
  showToast(`The preview of ${artifactPath} tried to ${what} without a click. Nothing was opened.`, 'error');
}

/** Download a workspace file from the shell's own origin, where the browser
 *  honours `download`. `name` is the link's own suggestion, when it made one. */
function downloadDataFile(path: string, name: string): void {
  const anchor = document.createElement('a');
  anchor.href = lucidos.data.url(path);
  anchor.download = name || path.slice(path.lastIndexOf('/') + 1);
  anchor.rel = 'noopener';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
}
