// @vitest-environment jsdom
/**
 * The engine's browser tool runs these scripts on every page it opens, under
 * the user's logged-in browser profile. They click a cookie banner's accept
 * button. They must click nothing else: an "Accept" on an invitation, a friend
 * request or a terms page would be clicked as the user, with no approval.
 *
 * The scripts are `include_str!`d by `runtime/browser_consent.rs`, so no
 * engine test can run them. This suite runs them in jsdom, wrapped the same
 * way the engine wraps them: the shared context helper first, then the body.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const RUNTIME_DIR = resolve(here, '../../../lucidos-engine/src/runtime');
const read = (file: string): string => readFileSync(resolve(RUNTIME_DIR, file), 'utf-8');

const CONTEXT_JS = read('browser_consent_context.js');
const CLICK_JS = read('browser_consent_click.js');
const FRAME_PROBE_JS = read('browser_consent_frame_probe.js');

function runClickPass(): string {
  return new Function(`${CONTEXT_JS}\n${CLICK_JS}`)() as string;
}

function runFrameProbe(): Record<string, unknown> {
  const json = new Function(`${CONTEXT_JS}\n${FRAME_PROBE_JS}`)();
  return JSON.parse(json as string);
}

/** Renders `html` and records which buttons receive a click. */
function render(html: string): string[] {
  document.body.innerHTML = html;
  const clicked: string[] = [];
  for (const btn of Array.from(document.querySelectorAll('button'))) {
    btn.addEventListener('click', () => clicked.push(btn.textContent ?? ''));
  }
  return clicked;
}

const LONE_ACCEPT_PAGES = {
  'an invitation with a bare Accept': '<main><h1>Team invite</h1><button>Accept</button></main>',
  'a terms page with I agree': '<button>I agree</button>',
  'a friend request with Accept all': '<section><p>3 friend requests</p><button>Accept all</button></section>',
  'an Accept styled with a Tailwind tracking class':
    '<main><h1>Team invite</h1><button class="px-4 uppercase tracking-wide">Accept</button></main>',
  'a terms update that names the Privacy Policy':
    '<div class="modal"><p>We updated our Terms of Service and Privacy Policy.</p><button>I agree</button></div>',
};

const CONSENT_BANNER =
  '<div id="cookie-banner"><p>We use cookies to improve your experience.</p><button>Accept all</button></div>';

describe('browser consent scripts', () => {
  let restore: () => void;

  beforeEach(() => {
    // jsdom does no layout and has no innerText. Give every element a
    // visible box and read innerText from the text content.
    const proto = HTMLElement.prototype;
    const rect = Element.prototype.getBoundingClientRect;
    const innerText = Object.getOwnPropertyDescriptor(proto, 'innerText');
    Element.prototype.getBoundingClientRect = () =>
      ({ left: 100, top: 100, width: 120, height: 40, right: 220, bottom: 140, x: 100, y: 100 }) as DOMRect;
    Object.defineProperty(proto, 'innerText', {
      configurable: true,
      get(this: HTMLElement) {
        return this.textContent;
      },
    });
    restore = () => {
      Element.prototype.getBoundingClientRect = rect;
      if (innerText) Object.defineProperty(proto, 'innerText', innerText);
      else delete (proto as { innerText?: string }).innerText;
      document.body.innerHTML = '';
    };
  });

  afterEach(() => restore());

  describe('main-frame click pass', () => {
    for (const [name, html] of Object.entries(LONE_ACCEPT_PAGES)) {
      it(`does not click ${name}`, () => {
        const clicked = render(html);
        expect(runClickPass()).toMatch(/^not found/);
        expect(clicked).toEqual([]);
      });
    }

    it('reports no page text when it finds nothing, since the engine logs it', () => {
      render('<button>Signed in as user@example.com</button>');
      const result = runClickPass();
      expect(result).toMatch(/^not found/);
      expect(result).not.toContain('example.com');
    });

    it('clicks Accept all inside a cookie banner', () => {
      const clicked = render(CONSENT_BANNER);
      expect(runClickPass()).toBe('clicked: accept all');
      expect(clicked).toEqual(['Accept all']);
    });

    it('clicks a bare Accept inside a consent dialog', () => {
      const clicked = render('<div class="consent-dialog"><p>We use cookies.</p><button>Accept</button></div>');
      expect(runClickPass()).toBe('clicked fallback: accept');
      expect(clicked).toEqual(['Accept']);
    });

    it('steps past an SVG Accept all to reach the banner', () => {
      const clicked = render(
        `<svg><g role="button" class="icon"><text>Accept all</text></g></svg>${CONSENT_BANNER}`,
      );
      expect(runClickPass()).toBe('clicked: accept all');
      expect(clicked).toEqual(['Accept all']);
    });
  });

  describe('frame probe', () => {
    for (const [name, html] of Object.entries(LONE_ACCEPT_PAGES)) {
      it(`finds nothing on ${name}`, () => {
        render(html);
        expect(runFrameProbe()).toMatchObject({ debug: true });
      });
    }

    it('finds Accept all inside a cookie banner', () => {
      render(CONSENT_BANNER);
      expect(runFrameProbe()).toMatchObject({ text: 'accept all' });
    });

    it("finds Accept all in a CMP frame's message", () => {
      render(
        '<div class="message-container"><p>We and our partners store information on your device, such as cookies.</p>' +
          '<div class="message-row"><button class="message-button">Accept all</button></div></div>',
      );
      expect(runFrameProbe()).toMatchObject({ text: 'accept all' });
    });
  });
});
