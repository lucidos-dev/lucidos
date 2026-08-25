// @vitest-environment jsdom
// The sanitizer runs on a real DOM. The default `node` environment has none,
// and DOMPurify would pass its input straight back.

// A notification body is agent-authored markdown, rendered through the same
// `renderMarkdown` + `linkifyPaths` pipeline as a chat turn. So a
// `[name](trigger:<id>)` the agent writes into `send_notification` becomes an
// `a.trigger-link` here, exactly as it does in the transcript. The body's own
// click handler has to claim it. Left unclaimed the anchor keeps its
// `href="#"` and the tap does nothing, which is the reported bug moved to a
// different surface.
//
// A source-level pin, matching this directory's existing contract test: the
// handler is a closure over component state and cannot be imported. The
// rendering half is covered for real, since that half IS importable.
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';
import { linkifyPaths } from '../../utils/linkifyPaths';
import { renderMarkdown } from '../../utils/renderMarkdown';

const here: string = dirname(fileURLToPath(import.meta.url));
const componentSource = readFileSync(resolve(here, 'NotificationDetailInline.tsx'), 'utf-8');

function bodyClickSource(): string {
  const m = componentSource.match(/function handleBodyClick[\s\S]*?\n  \}\n/);
  expect(m, 'handleBodyClick not found in NotificationDetailInline.tsx').not.toBeNull();
  return m![0];
}

describe('a trigger link in a notification body', () => {
  it('renders as the a.trigger-link the handler looks for', () => {
    const html = linkifyPaths(
      renderMarkdown('[Nightly digest](trigger:3f9b21c4-0a7e)'),
      [],
      [],
    );
    expect(html).toContain('class="trigger-link"');
    expect(html).toContain('data-trigger-id="3f9b21c4-0a7e"');
  });

  it('is claimed by handleBodyClick and routed through navigateToTrigger', () => {
    const body = bodyClickSource();
    expect(body).toContain("closest<HTMLAnchorElement>('a.trigger-link')");
    // The same call the Open trigger button makes, source string included, so
    // a genuine miss toast says where the navigate came from.
    expect(body).toContain("navigateToTrigger(linkedTriggerId, 'a notification')");
    expect(body).toContain('e.preventDefault()');
  });

  it('reports an id-less trigger link instead of silently doing nothing', () => {
    expect(bodyClickSource()).toMatch(/if \(!linkedTriggerId\) \{[\s\S]*?showToast\(/);
  });

  it('still claims a.app-link', () => {
    // The trigger arm is added ahead of the app arm; neither may displace the
    // other, and an anchor never carries both classes.
    expect(bodyClickSource()).toContain("closest<HTMLAnchorElement>('a.app-link')");
  });
});
