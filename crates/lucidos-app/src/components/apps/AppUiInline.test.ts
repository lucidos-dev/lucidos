import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';


const here: string = dirname(fileURLToPath(import.meta.url));
const src = readFileSync(resolve(here, 'AppUiInline.tsx'), 'utf-8')
  + readFileSync(resolve(here, 'AppFrame.tsx'), 'utf-8');
const css = readFileSync(resolve(here, '../../styles/panels/previews.css'), 'utf-8');
const mobileCss = readFileSync(resolve(here, '../../styles/mobile.css'), 'utf-8');

describe('app frame load cover', () => {
  it('mounts a cover element alongside the iframe', () => {
    expect(src).toMatch(/app-ui-cover/);
  });

  it('clears the cover on the frame load event, not on a timer alone', () => {
    expect(src).toMatch(/onLoad=\{\(e\) => \{\s*setLoaded\(true\);/);
  });

  it('hands the loaded frame the shortcut bindings, so the SDK cancels their browser defaults', () => {
    expect(src).toMatch(/pushKeybindingsToFrame\(e\.currentTarget as HTMLIFrameElement\)/);
  });

  it('tells the loaded frame whether a wheel can be a zoom, so a Cmd + wheel over it scales the UI', () => {
    expect(src).toMatch(/pushScaleWheelArmedToFrame\(e\.currentTarget as HTMLIFrameElement\)/);
  });

  it('hands the loaded frame the shell\'s appearance, so a change since the seed shows', () => {
    expect(src).toMatch(/pushAppearanceToFrame\(e\.currentTarget as HTMLIFrameElement\)/);
  });

  it('keeps the cover mounted through its fade-out, at any animation speed', () => {
    expect(src).toMatch(
      /useLingeringFlag\(!revealed, scaledDurationMs\(COVER_FADE_MS\) \+ COVER_FADE_SLACK_MS\)/,
    );
    expect(src).toMatch(/const COVER_FADE_MS = 200;/);
  });

  it('re-covers the frame on an app switch', () => {
    expect(src).toMatch(/key=\{`\$\{refreshKey\}:\$\{splitFrameSrc\(frameSrc\)\.doc\}`\}/);
    expect(src).toMatch(/const \[loaded, setLoaded\] = useState\(false\)/);
  });

  it('reveals the frame anyway if load never fires', () => {
    expect(src).toMatch(/setTimeout\(\(\) => setFused\(true\), revealFuseMs\(reveal\)\)/);
  });

  it('reveals through the shared decision, fed by load, ready and the fuse', () => {
    expect(src).toMatch(/const revealed = appFrameRevealed\(reveal, signals\)/);
    expect(src).toMatch(/addEventListener\(APP_FRAME_READY_EVENT, onReady\)/);
    expect(src).toMatch(/useLayoutEffect\(\(\) => \{\s*const iframe = iframeRef\.current;\s*if \(!iframe\) return;\s*const onReady/);
    expect(src).toMatch(/class=\{`app-ui-cover\$\{revealed \? ' is-clearing' : ''\}`\}/);
  });

  it('paints the cover with the theme background, opaque and click-through', () => {
    const rule = css.match(/\.app-ui-cover\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/background:\s*var\(--bg-primary\)/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    expect(rule).toMatch(/opacity:\s*1/);
  });

  it('positions the cover over the frame', () => {
    expect(css).toMatch(/\.app-ui-inline\s*\{[^}]*position:\s*relative/);
    expect(css).toMatch(/\.app-ui-cover\s*\{[^}]*position:\s*absolute/);
  });
});

describe('app panel refresh', () => {
  it('caps its wait at the open app\'s own reveal fuse', () => {
    expect(src).toMatch(/setTimeout\(settle, revealFuseMs\(reveal\) \+ REFRESH_SETTLE_SLACK_MS\)/);
    expect(src).toMatch(/\(\) => refreshOpenApp\(reveal\)/);
  });
});

describe('app frame load bar', () => {
  it('is delay-gated, so a fast open shows no bar', () => {
    expect(src).toMatch(/const barShown = useDelayedFlag\(appFrameLoading\(reveal, signals\)\)/);
  });

  it('lingers through its fade-out at any animation speed', () => {
    expect(src).toMatch(
      /useLingeringFlag\(barShown, scaledDurationMs\(COVER_FADE_MS\) \+ COVER_FADE_SLACK_MS\)/,
    );
  });

  it('is opaque from its first frame and fades only on the way out', () => {
    const rule = css.match(/\.app-ui-load-bar\s*\{[^}]*\}/)?.[0] ?? '';
    expect(rule).toMatch(/opacity:\s*1/);
    expect(rule).toMatch(/pointer-events:\s*none/);
    expect(css).toMatch(/\.app-ui-load-bar\.is-clearing\s*\{[^}]*opacity:\s*0/);
    expect(css).not.toMatch(/@keyframes app-ui-load-bar-[a-z-]*\s*\{[^}]*opacity/);
  });

  it('is not painted over by the phone header\'s fade', () => {
    expect(mobileCss).toMatch(
      /:root:has\(\.app-ui-load-bar\) \.app-header\[data-mobile-view="content"\]::after\s*\{\s*opacity:\s*0;/,
    );
  });

  it('names itself to assistive tech as a progress bar', () => {
    expect(src).toMatch(/role="progressbar"/);
    expect(src).toMatch(/aria-label="Loading app"/);
  });
});

describe('app frame navigation', () => {
  const effect = src.match(/const iframe = iframeRef\.current;[\s\S]*?\}, \[src\]\);/)?.[0] ?? '';

  it('a change of DOCUMENT remounts the frame, rather than driving it', () => {
    expect(src).toMatch(/key=\{`\$\{refreshKey\}:\$\{splitFrameSrc\(frameSrc\)\.doc\}`\}/);
    expect(src).not.toContain('navigateAppIframe');
  });

  it('hands a fragment change to setAppFrameHash and raises NO cover', () => {
    expect(effect, 'navigation effect not found in AppUiInline.tsx').not.toBe('');
    expect(effect).toContain('setAppFrameHash(iframe, next.fragment)');
    expect(effect).not.toContain('setLoaded(false)');
    expect(effect).toMatch(/if \(next\.fragment\)/);
  });

  it('a remount re-covers by construction, since the new frame starts uncovered', () => {
    expect(src).toMatch(/const \[loaded, setLoaded\] = useState\(false\)/);
  });
});
