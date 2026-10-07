/* GENERATED from packages/lucidos-sdk/src/previewFind/ by previewFind.build.mjs.
   Do not edit: run `npm run build` in packages/lucidos-sdk. */
"use strict";
var __lucidosPreviewFind = (() => {
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

  // src/previewFind/previewFind.ts
  var previewFind_exports = {};
  __export(previewFind_exports, {
    serve: () => serveFind
  });

  // src/find.ts
  var ALL_HIGHLIGHT = "lucidos-find";
  var CURRENT_HIGHLIGHT = "lucidos-find-current";
  var MAX_FIND_MATCHES = 2e3;
  var SKIPPED_TAGS = /* @__PURE__ */ new Set([
    "SCRIPT",
    "STYLE",
    "NOSCRIPT",
    "TEMPLATE",
    "TEXTAREA",
    "SELECT",
    "OPTION"
  ]);
  var BLOCK_TAGS = /* @__PURE__ */ new Set([
    "ADDRESS",
    "ARTICLE",
    "ASIDE",
    "BLOCKQUOTE",
    "DD",
    "DETAILS",
    "DIV",
    "DL",
    "DT",
    "FIGCAPTION",
    "FIGURE",
    "FOOTER",
    "H1",
    "H2",
    "H3",
    "H4",
    "H5",
    "H6",
    "HEADER",
    "HR",
    "LI",
    "MAIN",
    "NAV",
    "OL",
    "P",
    "PRE",
    "SECTION",
    "SUMMARY",
    "TABLE",
    "TBODY",
    "TD",
    "TFOOT",
    "TH",
    "THEAD",
    "TR",
    "UL"
  ]);
  var BLOCK_BREAK = "\0";
  function queryPattern(query) {
    const words = query.trim().split(/\s+/).filter(Boolean);
    if (words.length === 0) return null;
    const escaped = words.map((w) => w.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
    return new RegExp(escaped.join("\\s+"), "giu");
  }
  function matchSpans(text, pattern, limit) {
    const spans = [];
    for (const m of text.matchAll(pattern)) {
      if (spans.length >= limit) break;
      spans.push([m.index, m.index + m[0].length]);
    }
    return spans;
  }
  function stepIndex(current, step, total) {
    if (total === 0) return 0;
    return (current + step + total) % total;
  }
  function isBlock(el, layout, cache) {
    let known = cache.get(el);
    if (known === void 0) {
      known = layout === "markup" ? BLOCK_TAGS.has(el.tagName) : !getComputedStyle(el).display.startsWith("inline");
      cache.set(el, known);
    }
    return known;
  }
  function blockOf(el, root, layout, cache) {
    let at = el;
    while (at && at !== root && !isBlock(at, layout, cache)) at = at.parentElement;
    return at ?? root;
  }
  function isVisible(el, layout, cache) {
    if (layout === "markup") return true;
    let known = cache.get(el);
    if (known === void 0) {
      known = typeof el.checkVisibility === "function" ? el.checkVisibility({ visibilityProperty: true }) : true;
      cache.set(el, known);
    }
    return known;
  }
  function collectPageText(root = document.body, layout = "rendered") {
    const visible = /* @__PURE__ */ new Map();
    const blocks = /* @__PURE__ */ new Map();
    const walker = root.ownerDocument.createTreeWalker(root, NodeFilter.SHOW_TEXT | NodeFilter.SHOW_ELEMENT, {
      acceptNode(node) {
        if (node.nodeType === Node.ELEMENT_NODE) {
          const tag = node.tagName;
          if (tag === "BR") return NodeFilter.FILTER_ACCEPT;
          return SKIPPED_TAGS.has(tag) ? NodeFilter.FILTER_REJECT : NodeFilter.FILTER_SKIP;
        }
        const parent = node.parentElement;
        if (!parent || !node.nodeValue) return NodeFilter.FILTER_REJECT;
        return isVisible(parent, layout, visible) ? NodeFilter.FILTER_ACCEPT : NodeFilter.FILTER_REJECT;
      }
    });
    let text = "";
    const segments = [];
    let lastBlock = null;
    for (let node = walker.nextNode(); node; node = walker.nextNode()) {
      if (node.nodeType === Node.ELEMENT_NODE) {
        text += "\n";
        continue;
      }
      const textNode = node;
      const block = blockOf(textNode.parentElement, root, layout, blocks);
      if (lastBlock && block !== lastBlock) text += BLOCK_BREAK;
      lastBlock = block;
      segments.push({ node: textNode, start: text.length });
      text += textNode.nodeValue;
    }
    return { text, segments };
  }
  function segmentAt(segments, at, atEnd) {
    let lo = 0;
    let hi = segments.length - 1;
    while (lo < hi) {
      const mid = lo + hi + 1 >> 1;
      const start = segments[mid].start;
      if (start < at || !atEnd && start === at) lo = mid;
      else hi = mid - 1;
    }
    return segments[lo];
  }
  function spanToRange(page, [from, to]) {
    const a = segmentAt(page.segments, from, false);
    const b = segmentAt(page.segments, to, true);
    const range = document.createRange();
    range.setStart(a.node, from - a.start);
    range.setEnd(b.node, to - b.start);
    return range;
  }
  function frameStylesHighlights() {
    return highlightApiPresent() && getComputedStyle(document.documentElement).getPropertyValue("--find-highlights").trim() === "styled";
  }
  function highlightApiPresent() {
    return typeof CSS !== "undefined" && "highlights" in CSS && typeof Highlight === "function";
  }
  function firstInView(ranges, root) {
    const top = Math.max(0, root.getBoundingClientRect().top);
    const index = ranges.findIndex((r) => r.getBoundingClientRect().bottom >= top);
    return index === -1 ? 0 : index;
  }
  function scrolls(el) {
    const style = getComputedStyle(el);
    const canScroll = /auto|scroll|overlay/.test(`${style.overflowY} ${style.overflowX}`);
    return canScroll && (el.scrollHeight > el.clientHeight || el.scrollWidth > el.clientWidth);
  }
  function centring(spanStart, spanSize, boxStart, boxSize) {
    const visible = spanStart >= boxStart && spanStart + spanSize <= boxStart + boxSize;
    return visible ? 0 : spanStart - boxStart - (boxSize - spanSize) / 2;
  }
  function revealRange(range) {
    for (let el = range.startContainer.parentElement; el && el !== document.documentElement; el = el.parentElement) {
      if (!scrolls(el)) continue;
      const box = el.getBoundingClientRect();
      const r2 = range.getBoundingClientRect();
      el.scrollTop += centring(r2.top, r2.height, box.top, box.height);
      el.scrollLeft += centring(r2.left, r2.width, box.left, box.width);
    }
    const r = range.getBoundingClientRect();
    const dy = centring(r.top, r.height, 0, window.innerHeight);
    const dx = centring(r.left, r.width, 0, window.innerWidth);
    if (dx || dy) window.scrollBy(dx, dy);
  }
  function setHighlight(name, ranges) {
    CSS.highlights.get(name)?.clear();
    if (ranges.length > 0) CSS.highlights.set(name, new Highlight(...ranges));
    else CSS.highlights.delete(name);
  }
  function createPainter(highlights) {
    let selectionOwned = false;
    return {
      paint(ranges, current) {
        const at = current >= 0 ? ranges[current] : void 0;
        if (highlights()) {
          setHighlight(ALL_HIGHLIGHT, ranges);
          setHighlight(CURRENT_HIGHLIGHT, at ? [at] : []);
          return;
        }
        const selection = window.getSelection();
        if (!selection) return;
        selection.removeAllRanges();
        if (at) selection.addRange(at);
        selectionOwned = !!at;
      },
      clear() {
        if (typeof CSS !== "undefined" && "highlights" in CSS) {
          setHighlight(ALL_HIGHLIGHT, []);
          setHighlight(CURRENT_HIGHLIGHT, []);
        }
        if (selectionOwned) {
          window.getSelection()?.removeAllRanges();
          selectionOwned = false;
        }
      }
    };
  }
  function createFinder(root, painter) {
    let session = null;
    const none = { total: 0, current: 0, capped: false };
    const clear = () => {
      session = null;
      painter.clear();
    };
    return {
      clear,
      find(query, step) {
        const pattern = queryPattern(query);
        const el = root();
        if (!pattern || !el) {
          clear();
          return none;
        }
        const page = collectPageText(el);
        const spans = matchSpans(page.text, pattern, MAX_FIND_MATCHES + 1);
        const capped = spans.length > MAX_FIND_MATCHES;
        const ranges = spans.slice(0, MAX_FIND_MATCHES).map((s) => spanToRange(page, s));
        const total = ranges.length;
        let current;
        if (session?.query !== query) current = firstInView(ranges, el);
        else if (step) current = stepIndex(session.current, step, total);
        else current = Math.min(session.current, Math.max(total - 1, 0));
        session = { query, current };
        painter.paint(ranges, total > 0 ? current : -1);
        if (total > 0) revealRange(ranges[current]);
        return { total, current: total > 0 ? current + 1 : 0, capped };
      }
    };
  }
  var frameFinder = createFinder(() => document.body, createPainter(frameStylesHighlights));
  var find = (query, step) => frameFinder.find(query, step);
  var clearFind = () => frameFinder.clear();
  function serveFind(args) {
    const a = args ?? {};
    if (a.clear === true) {
      clearFind();
      return { total: 0, current: 0, capped: false };
    }
    if (typeof a.query !== "string") throw new Error("find needs a query");
    const step = a.step === 1 || a.step === -1 ? a.step : void 0;
    return find(a.query, step);
  }
  return __toCommonJS(previewFind_exports);
})();
