/* GENERATED from packages/lucidos-sdk/src/paneSwipe.ts by previewBundles.build.mjs.
   Do not edit: run `npm run build` in packages/lucidos-sdk. */
"use strict";
var __lucidosPreviewSwipe = (() => {
  var __defProp = Object.defineProperty;
  var __getOwnPropDesc = Object.getOwnPropertyDescriptor;
  var __getOwnPropNames = Object.getOwnPropertyNames;
  var __hasOwnProp = Object.prototype.hasOwnProperty;
  var __export = (target, all) => {
    for (var name in all)
      __defProp(target, name, { get: all[name], enumerable: true });
  };
  var __copyProps = (to, from, except, desc) => {
    if (from && typeof from === "object" || typeof from === "function") {
      for (let key of __getOwnPropNames(from))
        if (!__hasOwnProp.call(to, key) && key !== except)
          __defProp(to, key, { get: () => from[key], enumerable: !(desc = __getOwnPropDesc(from, key)) || desc.enumerable });
    }
    return to;
  };
  var __toCommonJS = (mod) => __copyProps(__defProp({}, "__esModule", { value: true }), mod);

  // src/paneSwipe.ts
  var paneSwipe_exports = {};
  __export(paneSwipe_exports, {
    APP_SWIPE_END_MESSAGE_TYPE: () => APP_SWIPE_END_MESSAGE_TYPE,
    APP_SWIPE_MESSAGE_TYPE: () => APP_SWIPE_MESSAGE_TYPE,
    SwipeTouch: () => SwipeTouch,
    claimsHorizontalPan: () => claimsHorizontalPan,
    horizontalDragClaimed: () => horizontalDragClaimed,
    installAppPaneSwipe: () => installAppPaneSwipe,
    installFramePaneSwipe: () => installFramePaneSwipe,
    pageScrollsSideways: () => pageScrollsSideways,
    trackPaneSwipe: () => trackPaneSwipe
  });

  // src/textEntry.ts
  var NON_TEXT_INPUT_TYPES = /* @__PURE__ */ new Set([
    "button",
    "submit",
    "reset",
    "image",
    "file",
    "checkbox",
    "radio",
    "range",
    "color",
    "hidden"
  ]);
  function isTextEntryField(el) {
    if (el.localName === "textarea") return true;
    if (el.localName === "input") return !NON_TEXT_INPUT_TYPES.has(el.type);
    return false;
  }

  // src/paneSwipe.ts
  var LOCK_THRESHOLD = 8;
  var MIN_SWIPE_DISTANCE = 30;
  var FAST_SWIPE_VELOCITY = 0.3;
  var APP_SWIPE_MESSAGE_TYPE = "lucidos:app:swipe";
  var APP_SWIPE_END_MESSAGE_TYPE = "lucidos:app:swipe-end";
  var SwipeTouch = class {
    constructor() {
      this._startX = 0;
      this._startY = 0;
      this._startTime = 0;
      this._tracking = false;
      this._direction = null;
      this._dx = 0;
      /** Which way the finger was going when the direction locked. Frozen there,
       *  because the threshold compensation below is one fixed offset for the whole
       *  gesture. Re-reading the sign per frame flips the offset the moment the
       *  finger drags back past its start, which reports +7 for a 1px move LEFT. */
      this._lockSign = 0;
    }
    /** Record start of a new touch. */
    start(x, y) {
      this._startX = x;
      this._startY = y;
      this._startTime = Date.now();
      this._tracking = true;
      this._direction = null;
      this._dx = 0;
      this._lockSign = 0;
    }
    /** Process a touch move. Returns the horizontal delta (px) if tracking
     *  horizontally, or null if vertical/undecided/not tracking. */
    move(x, y) {
      if (!this._tracking) return null;
      const dx = x - this._startX;
      const dy = y - this._startY;
      if (this._direction === null) {
        if (Math.abs(dx) < LOCK_THRESHOLD && Math.abs(dy) < LOCK_THRESHOLD) return null;
        this._direction = Math.abs(dx) >= Math.abs(dy) ? "horizontal" : "vertical";
        this._lockSign = Math.sign(dx);
      }
      if (this._direction === "vertical") return null;
      const adjusted = dx - this._lockSign * LOCK_THRESHOLD;
      this._dx = adjusted;
      return adjusted;
    }
    /** End the touch. Returns the pane delta: -1 (prev), 0 (snap back), +1 (next).
     *  Only non-zero when the gesture was a confirmed horizontal swipe. */
    end(paneWidth) {
      if (!this._tracking || this._direction !== "horizontal") {
        this._tracking = false;
        return 0;
      }
      this._tracking = false;
      const elapsed = Math.max(1, Date.now() - this._startTime);
      const velocity = Math.abs(this._dx) / elapsed;
      const fastSwipe = velocity > FAST_SWIPE_VELOCITY && Math.abs(this._dx) > MIN_SWIPE_DISTANCE;
      const farDrag = Math.abs(this._dx) > paneWidth / 3;
      if (fastSwipe || farDrag) {
        return this._dx > 0 ? -1 : 1;
      }
      return 0;
    }
    /** Whether a horizontal swipe is currently being tracked. */
    get isHorizontal() {
      return this._tracking && this._direction === "horizontal";
    }
    /** Cancel tracking without producing a result. */
    cancel() {
      this._tracking = false;
      this._direction = null;
      this._dx = 0;
      this._lockSign = 0;
    }
  };
  function claimsHorizontalPan(touchAction) {
    if (touchAction === "" || touchAction === "auto" || touchAction === "manipulation") return false;
    return !touchAction.split(/\s+/).includes("pan-x");
  }
  function horizontalDragClaimed(target, styleOf) {
    for (let node = target; node; node = node.parentElement) {
      if (node.localName === "input" && node.type === "range") return true;
      const style = styleOf(node);
      if (!style) continue;
      if (claimsHorizontalPan(style.touchAction)) return true;
      const overflows = (node.scrollWidth ?? 0) > (node.clientWidth ?? 0);
      if (overflows && (style.overflowX === "auto" || style.overflowX === "scroll")) return true;
    }
    return false;
  }
  var computedStyleOf = (node) => {
    if (typeof getComputedStyle !== "function" || typeof Element === "undefined") return null;
    return node instanceof Element ? getComputedStyle(node) : null;
  };
  function textFieldFocused() {
    const el = typeof document === "undefined" ? null : document.activeElement;
    if (!el) return false;
    return isTextEntryField(el) || el.isContentEditable === true;
  }
  function pageScrollsSideways(doc, overflowXOf) {
    const root = doc.scrollingElement;
    if (!root || root.scrollWidth <= root.clientWidth) return false;
    return [doc.documentElement, doc.body].every((el) => !el || !/^(hidden|clip)$/.test(overflowXOf(el)));
  }
  function onlyTouch(e) {
    const touches = e.touches;
    return touches && touches.length === 1 ? touches[0] : null;
  }
  function screenPoint(t) {
    return [t.screenX, t.screenY];
  }
  function trackPaneSwipe(surface, handlers, env) {
    const touch = new SwipeTouch();
    const abandon = () => {
      if (touch.isHorizontal) handlers.onRelease(0);
      touch.cancel();
    };
    const onStart = (e) => {
      abandon();
      const t = onlyTouch(e);
      if (!t || e.defaultPrevented || env.textFieldFocused()) return;
      if (env.pageScrollsSideways() || horizontalDragClaimed(e.target, env.styleOf)) return;
      touch.start(...screenPoint(t));
    };
    const onMove = (e) => {
      const t = onlyTouch(e);
      if (!t || e.defaultPrevented) {
        abandon();
        return;
      }
      const dx = touch.move(...screenPoint(t));
      if (dx === null) return;
      if (!e.cancelable) {
        abandon();
        return;
      }
      e.preventDefault();
      handlers.onDrag(dx);
    };
    const onEnd = () => {
      if (touch.isHorizontal) handlers.onRelease(touch.end(env.paneWidth()));
      touch.cancel();
    };
    const passive = { passive: true };
    const active = { passive: false };
    const trusted = (fn) => (e) => {
      if (e.isTrusted) fn(e);
    };
    const listeners = [
      ["touchstart", trusted(onStart), passive],
      ["touchmove", trusted(onMove), active],
      ["touchend", trusted(onEnd), passive],
      ["touchcancel", trusted(abandon), passive],
      // A frame that reloads mid-drag never sends its touchend.
      ["pagehide", trusted(abandon), passive]
    ];
    for (const [type, fn, options] of listeners) surface.addEventListener(type, fn, options);
    return () => {
      for (const [type, fn, options] of listeners) surface.removeEventListener(type, fn, options);
    };
  }
  function installFramePaneSwipe(win, handlers, { yieldToWidePage }) {
    return trackPaneSwipe(win, handlers, {
      // The frame is full-bleed in its pane, so its width is the pane's.
      paneWidth: () => win.innerWidth,
      textFieldFocused,
      styleOf: computedStyleOf,
      pageScrollsSideways: () => yieldToWidePage && pageScrollsSideways(win.document, (el) => win.getComputedStyle(el).overflowX)
    });
  }
  function installAppPaneSwipe() {
    if (typeof window === "undefined" || window.parent === window) return () => {
    };
    const post = (message) => window.parent.postMessage(message, "*");
    return installFramePaneSwipe(window, {
      onDrag: (dx) => post({ type: APP_SWIPE_MESSAGE_TYPE, dx }),
      onRelease: (paneDelta) => post({ type: APP_SWIPE_END_MESSAGE_TYPE, paneDelta })
    }, { yieldToWidePage: false });
  }
  return __toCommonJS(paneSwipe_exports);
})();
