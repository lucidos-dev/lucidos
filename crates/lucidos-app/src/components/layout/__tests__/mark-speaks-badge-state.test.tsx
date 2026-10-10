// @vitest-environment jsdom
/**
 * The state badge is click-through, so a pointer or a screen reader lands on
 * the mark. The mark therefore speaks what the badge stands for.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { BrandMenuButton } from '../HeaderMark';
import { engineBuilding } from '../../../store/store';

describe('the mark speaks the state badge', () => {
  let host: HTMLDivElement | null = null;

  afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    engineBuilding.value = false;
  });

  it('names the job in flight in its label and its tooltip', () => {
    engineBuilding.value = true;
    host = document.createElement('div');
    document.body.appendChild(host);
    render(<BrandMenuButton placement="brand" />, host);
    const mark = host.querySelector('[data-role="brand-menu-toggle"]')!;
    expect(mark.getAttribute('aria-label')).toContain('Building new version');
    expect(mark.getAttribute('data-tooltip')).toContain('Building new version');
    expect(host.querySelector('.brand-badge')?.getAttribute('data-tooltip')).toBeNull();
  });
});
