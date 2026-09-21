import { test, expect } from './fixtures';
import { createIframeAppFixture } from './db-helpers';
import { gotoWithRetry } from './helpers';

// The reported bug: every Copy button in every app was dead in v0.39.0, and it
// said nothing. ADR 0227 gave the app frame an opaque origin. The
// `clipboard-write` permissions-policy feature defaults to an allowlist of
// `self`, which an opaque origin does not match, so Chromium denied it inside
// the frame. App code does not await `writeText`, so the app toasted "Copied"
// on a copy that never happened.
//
// The unit floor is `components/apps/__tests__/app-frame-allow.test.ts`. This
// is the half only a browser can say: the attribute reaches a real frame and
// the copy really goes through. So it writes to the machine's clipboard, and
// the probe text says so for anyone who pastes after a run.
//
// Chromium alone, and the filename is the whole mechanism. `-desktop` drops
// both mobile projects, which are the only WebKit one and the same Chromium at
// 375px. Neither could witness this: WebKit resolves `writeText` at an opaque
// origin with and without the attribute, and a narrower viewport says nothing
// about a permissions policy. What `mobile` does add is a shell that parks the
// frame off-screen in the swipe container, so the click waits out its timeout.
// Reasons in `docs/plans/2026-09-21-the-app-frame-delegates-clipboard-write.md`.

const APP_ID = 'e2e-copy-app';
const PROBE = 'lucidos e2e clipboard probe';

/** Undefined until `beforeAll` builds it, so a `beforeAll` that throws part way
 *  still leaves the `afterAll` below able to run. */
let fixture: { dir: string; cleanup: () => void } | undefined;

test.describe('an app frame can copy', () => {
  test.beforeAll(() => {
    fixture = createIframeAppFixture(APP_ID, {
      manifest: { id: APP_ID, name: 'Copy app', description: 'e2e fixture' },
      html: `<!DOCTYPE html>
<html>
<head><meta charset="UTF-8"><title>Copy app</title></head>
<body>
<div id="ready">ready</div>
<button id="copy">Copy</button>
<div id="outcome">untried</div>
<script>
  // Unguarded on purpose: this is how app authors write a Copy button, and
  // the silence is half the defect. The outcome lands in the DOM so the spec
  // reads the browser's own verdict rather than inferring one.
  document.getElementById('copy').addEventListener('click', function () {
    navigator.clipboard.writeText(${JSON.stringify(PROBE)}).then(
      function () { document.getElementById('outcome').textContent = 'resolved'; },
      function (err) { document.getElementById('outcome').textContent = 'rejected ' + err.name + ': ' + err.message; }
    );
  });
</script>
</body>
</html>
`,
      js: '',
    });
  });

  test.afterAll(() => {
    fixture?.cleanup();
  });

  /** Seeding `app-window-open` makes `loadApps()` restore the app through the
   *  real `AppUiInline`. So the iframe carries production's own `allow` rather
   *  than a string copied into this file. */
  async function openAppOnLoad(page: import('@playwright/test').Page): Promise<void> {
    await page.addInitScript((id) => {
      // An init script runs in EVERY frame, and the app frame's storage throws
      // because it is isolated. Only the top frame is the shell.
      if (window.parent !== window) return;
      localStorage.setItem('app-window-open', id);
    }, APP_ID);
  }

  test('a Copy button inside an app puts text on the clipboard', async ({ page }) => {
    // The permissions policy is one gate and the user permission is another.
    // Headless Chromium has no window a user could have focused, so the second
    // gate needs granting. The first is what this spec measures, and no grant
    // can lift it: without the attribute the rejection still names the policy.
    await page.context().grantPermissions(['clipboard-write']);

    await openAppOnLoad(page);
    await gotoWithRetry(page, '/');

    const frameElement = page.locator('iframe[data-role="app-ui-frame"]:visible');
    await expect(frameElement).toHaveCount(1, { timeout: 15_000 });

    const allow = await frameElement.getAttribute('allow');
    expect(allow, 'the shipped frame must delegate clipboard-write').toContain('clipboard-write');
    expect(allow, 'reading the clipboard is deliberately not delegated').not.toContain('clipboard-read');

    const appFrame = page.frameLocator('iframe[data-role="app-ui-frame"]:visible');
    await expect(appFrame.locator('#ready')).toBeVisible({ timeout: 15_000 });

    await appFrame.locator('#copy').click();

    // On the regression this reads "rejected NotAllowedError: … blocked because
    // of a permissions policy …", so a failure names its own cause.
    await expect(appFrame.locator('#outcome')).toHaveText('resolved');
  });
});
