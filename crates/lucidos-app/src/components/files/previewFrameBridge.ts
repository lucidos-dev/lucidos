
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
  settlePreviewFind,
} from '../../utils/previewFrameProtocol';
import PREVIEW_FIND_SOURCE from '../../../../../packages/lucidos-sdk/dist/preview-find.js?raw';
import PREVIEW_SWIPE_SOURCE from '../../../../../packages/lucidos-sdk/dist/preview-swipe.js?raw';
import { forwardPreviewSwipe } from '../../store/actions/frame-swipe-bridge';
import {
  PREVIEW_HOST_SCHEMES,
  type PreviewLinkAction,
  classifyPreviewLink,
  previewLinkContext,
  resolvePreviewRelativePath,
  runPreviewLinkAction,
} from './previewIframeLinks';

export const ARTIFACT_PREVIEW_SANDBOX =
  'allow-scripts allow-forms allow-modals allow-popups allow-downloads';

export const ARTIFACT_PREVIEW_ALLOW = 'autoplay; fullscreen *; clipboard-write';

export function newBridgeNonce(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
}

export interface PreviewBridgeConfig {
  nonce: string;
  bindings: readonly Binding[];
  hostSchemes: readonly string[];
  paneSwipe: boolean;
}

export const PREVIEW_BRIDGE_SOURCE = String.raw`function (cfg, win, doc, finder, swiper) {
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
    } else if (data.kind === 'find' && typeof data.id === 'string') {
      var reply = null;
      try { reply = finder ? finder.serve(data.args) : null; } catch (err) { reply = null; }
      if (!data.id) return;
      post({ type: frameType, nonce: nonce, kind: 'find-result', id: data.id,
        total: reply ? reply.total : null, current: reply ? reply.current : null,
        capped: reply ? reply.capped : null });
    }
  });
  if (cfg.paneSwipe && swiper) {
    swiper.installFramePaneSwipe(win, {
      onDrag: function (dx) {
        post({ type: frameType, nonce: nonce, kind: 'swipe', dx: dx });
      },
      onRelease: function (paneDelta) {
        post({ type: frameType, nonce: nonce, kind: 'swipe-end', paneDelta: paneDelta });
      }
    }, { yieldToWidePage: true });
  }
}`;

export const PREVIEW_FIND_LOCAL = '__lucidosPreviewFind';
export const PREVIEW_SWIPE_LOCAL = '__lucidosPreviewSwipe';

export function previewBridgeScript(cfg: PreviewBridgeConfig): string {
  const json = JSON.stringify({
    nonce: cfg.nonce,
    bindings: cfg.bindings,
    mac: isMac,
    schemes: cfg.hostSchemes,
    paneSwipe: cfg.paneSwipe,
    frameType: PREVIEW_FRAME_MESSAGE,
    hostType: PREVIEW_HOST_MESSAGE,
  }).replace(/</g, '\\u003c');
  return `<script>(function(){${PREVIEW_FIND_SOURCE}\n;${PREVIEW_SWIPE_SOURCE}\n;`
    + `(${PREVIEW_BRIDGE_SOURCE})(${json}, window, document, ${PREVIEW_FIND_LOCAL}, ${PREVIEW_SWIPE_LOCAL});})();</script>`;
}

export function withPreviewBridge(html: string, cfg: PreviewBridgeConfig): string {
  const doctype = /^(?:\s|<!--[\s\S]*?-->)*<!doctype[^>]*>/i.exec(html);
  const at = doctype ? doctype[0].length : 0;
  return html.slice(0, at) + previewBridgeScript(cfg) + html.slice(at);
}

export function previewBridgeConfig(bindings: readonly Binding[], paneSwipe: boolean): PreviewBridgeConfig {
  return { nonce: newBridgeNonce(), bindings, hostSchemes: PREVIEW_HOST_SCHEMES, paneSwipe };
}

export type PreviewFrameMessage =
  | { kind: 'link'; href: string; baseUri: string | null }
  | { kind: 'download'; href: string; name: string; baseUri: string | null }
  | {
    kind: 'chord';
    chord: { key: string; metaKey: boolean; ctrlKey: boolean; shiftKey: boolean; altKey: boolean };
  }
  | { kind: 'fragment-missing'; id: string }
  | { kind: 'swipe'; dx: number }
  | { kind: 'swipe-end'; paneDelta: -1 | 0 | 1 }
  | { kind: 'find-result'; id: string; reply: unknown };

const MAX_TEXT = 8192;

function boundedString(value: unknown): value is string {
  return typeof value === 'string' && value.length <= MAX_TEXT;
}

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
    case 'swipe':
      if (typeof data.dx !== 'number' || !Number.isFinite(data.dx)) return null;
      return { kind: 'swipe', dx: data.dx };
    case 'swipe-end':
      return { kind: 'swipe-end', paneDelta: data.paneDelta === 1 || data.paneDelta === -1 ? data.paneDelta : 0 };
    case 'find-result':
      if (!boundedString(data.id)) return null;
      return {
        kind: 'find-result',
        id: data.id,
        reply: data.total === null ? null : { total: data.total, current: data.current, capped: data.capped },
      };
    default:
      return null;
  }
}

export interface PreviewFrameContext {
  artifactPath: string;
  declaresOwnBase: boolean;
  frameWindow: Window | null;
}

function shellHasUserActivation(): boolean | null {
  const activation = (navigator as Navigator & { userActivation?: { isActive: boolean } }).userActivation;
  return activation ? activation.isActive : null;
}

export function previewMayNavigate(activation: boolean | null): boolean {
  return activation !== false;
}

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
    case 'swipe':
      forwardPreviewSwipe(ctx.frameWindow, { kind: 'drag', dx: msg.dx });
      return;
    case 'swipe-end':
      forwardPreviewSwipe(ctx.frameWindow, { kind: 'release', paneDelta: msg.paneDelta });
      return;
    case 'find-result':
      settlePreviewFind(msg.id, msg.reply);
      return;
  }
}

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

function decodeSegments(path: string): string {
  return path.split('/').map((segment) => {
    let decoded: string;
    try {
      decoded = decodeURIComponent(segment);
    } catch {
      return segment;
    }
    return decoded === '.' || decoded === '..' || /[/\\]/.test(decoded) ? segment : decoded;
  }).join('/');
}

function refuseUnclicked(what: string, artifactPath: string): void {
  showToast(`The preview of ${artifactPath} tried to ${what} without a click. Nothing was opened.`, 'error');
}

function downloadDataFile(path: string, name: string): void {
  const anchor = document.createElement('a');
  anchor.href = lucidos.data.url(path);
  anchor.download = name || path.slice(path.lastIndexOf('/') + 1);
  anchor.rel = 'noopener';
  document.body.appendChild(anchor);
  anchor.click();
  anchor.remove();
}
