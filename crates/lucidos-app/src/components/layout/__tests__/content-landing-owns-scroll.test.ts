// @vitest-environment jsdom
/** A landing on a row owns the content pane's scroll, so the pane's scroll
 *  memory stands down for that open. Otherwise it restores a saved position
 *  over the landing (`e2e/shortcut-search-landing-desktop.spec.ts`). */
import { describe, it, expect, beforeEach } from 'vitest';
import { createContentLandingClaim } from '../contentLanding';
import { applyNavFocus, clearNavFocus } from '../../shared/focusMarker';
import { pluginScrollTarget, settingsScrollTarget, triggerScrollTarget } from '../../../store/store';

const SHORTCUTS = 'settings:keyboard-shortcuts';

let body: HTMLDivElement;
let claims: ReturnType<typeof createContentLandingClaim>;

beforeEach(() => {
  clearNavFocus();
  settingsScrollTarget.value = null;
  pluginScrollTarget.value = null;
  triggerScrollTarget.value = null;
  document.body.innerHTML = '';
  body = document.createElement('div');
  document.body.appendChild(body);
  claims = createContentLandingClaim();
});

function markRowIn(parent: HTMLElement) {
  const row = document.createElement('div');
  parent.appendChild(row);
  applyNavFocus(row);
}

describe('the content landing claim', () => {
  it('claims nothing with no landing', () => {
    expect(claims(SHORTCUTS, body)).toBe(false);
  });

  it('claims a settings sub-section while its target waits for its row', () => {
    settingsScrollTarget.value = 'shortcut:zoomReset';
    expect(claims(SHORTCUTS, body)).toBe(true);
  });

  it('ignores a settings target on Settings home and on other views', () => {
    settingsScrollTarget.value = 'shortcut:zoomReset';
    expect(claims('settings:main', body)).toBe(false);
    expect(claims('files', body)).toBe(false);
  });

  // They wait on data and can linger (a plugin row a filter hides), and the
  // view must still be placed meanwhile.
  it('ignores a plugin or trigger target still waiting', () => {
    pluginScrollTarget.value = 'hidden-plugin';
    triggerScrollTarget.value = 'some-trigger';
    expect(claims('plugins', body)).toBe(false);
    expect(claims('triggers', body)).toBe(false);
  });

  it('claims the view once a landing marked a row in the pane', () => {
    markRowIn(body);
    expect(claims(SHORTCUTS, body)).toBe(true);
  });

  // The dead-link rescue asks again seconds later. A reader who dismissed the
  // marker without scrolling must not be moved off the row.
  it('holds the claim after the marker is dismissed, for the same visit', () => {
    markRowIn(body);
    expect(claims(SHORTCUTS, body)).toBe(true);
    clearNavFocus();
    expect(claims(SHORTCUTS, body)).toBe(true);
  });

  it('drops the claim when the pane shows another view', () => {
    markRowIn(body);
    expect(claims(SHORTCUTS, body)).toBe(true);
    clearNavFocus();
    expect(claims('files', body)).toBe(false);
    expect(claims(SHORTCUTS, body)).toBe(false);
  });

  it('ignores a marker outside the pane, such as a transcript turn', () => {
    markRowIn(document.body);
    expect(claims(SHORTCUTS, body)).toBe(false);
  });
});
