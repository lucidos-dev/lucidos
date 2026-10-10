// @vitest-environment jsdom
// The list panels' collapsible section header: one toggle, actions beside it,
// and a collapse the panel remembers per device.
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { render } from 'preact';
import { SectionHeader, SectionHeaderContent } from '../SectionHeader';
import {
  collapsedChangesSectionIds,
  toggleChangesSectionCollapsed,
  collapsedThreadQueueSectionIds,
  toggleThreadQueueSectionCollapsed,
} from '../../../store/store';

describe('SectionHeader', () => {
  let host: HTMLElement;

  beforeEach(() => {
    host = document.createElement('div');
    document.body.appendChild(host);
  });

  afterEach(() => {
    render(null, host);
    host.remove();
  });

  it('toggles from one button that says whether the section is open', () => {
    let toggled = 0;
    render(<SectionHeader title="Set aside" count={2} collapsed={false} onToggle={() => { toggled++; }} />, host);
    const toggle = host.querySelector<HTMLButtonElement>('.list-section-toggle')!;
    expect(toggle.getAttribute('aria-expanded')).toBe('true');
    expect(toggle.querySelector('.section-label')?.textContent).toBe('Set aside');
    expect(toggle.querySelector('.section-count-open')?.textContent).toBe('2');
    toggle.click();
    expect(toggled).toBe(1);
  });

  it('marks a collapsed header, which carries the hairline', () => {
    render(<SectionHeader title="Queued" count={0} collapsed onToggle={() => {}} />, host);
    const header = host.querySelector('.list-section-title')!;
    expect(header.classList.contains('list-section-title-collapsible')).toBe(true);
    expect(header.classList.contains('collapsed')).toBe(true);
    expect(host.querySelector('.list-section-toggle')?.getAttribute('aria-expanded')).toBe('false');
  });

  it('leads the toggle with a chevron, before the icon, inside the element carrying aria-expanded', () => {
    render(<SectionHeader title="Queued" count={1} collapsed={false} onToggle={() => {}} />, host);
    const toggle = host.querySelector<HTMLButtonElement>('.list-section-toggle')!;
    const chevron = toggle.firstElementChild!;
    expect(chevron.classList.contains('section-chevron')).toBe(true);
    expect(chevron.getAttribute('aria-hidden')).toBe('true');
    expect(chevron.querySelector('svg')).not.toBeNull();
  });

  it('draws no chevron on a header that cannot collapse', () => {
    render(<SectionHeaderContent title="Results" />, host);
    expect(host.querySelector('.section-chevron')).toBeNull();
  });

  it('puts the actions beside the toggle, never inside it', () => {
    render(
      <SectionHeader
        title="Running"
        count="1/4"
        collapsed={false}
        onToggle={() => {}}
        actions={<button class="action-btn">Capacity policy</button>}
      />,
      host,
    );
    expect(host.querySelector('button button')).toBeNull();
    expect(host.querySelector('.list-section-actions > .action-btn')?.textContent).toBe('Capacity policy');
  });
});

describe('a list panel remembers its collapsed sections', () => {
  beforeEach(() => {
    localStorage.clear();
    collapsedChangesSectionIds.value = new Set();
    collapsedThreadQueueSectionIds.value = new Set();
  });

  it('stores the Changes panel sections per device', () => {
    toggleChangesSectionCollapsed('applied');
    expect(collapsedChangesSectionIds.value.has('applied')).toBe(true);
    expect(JSON.parse(localStorage.getItem('lucidos-collapsed-changes-sections')!)).toEqual(['applied']);
    toggleChangesSectionCollapsed('applied');
    expect(JSON.parse(localStorage.getItem('lucidos-collapsed-changes-sections')!)).toEqual([]);
  });

  it('stores the Thread queue sections per device', () => {
    toggleThreadQueueSectionCollapsed('queued');
    expect(collapsedThreadQueueSectionIds.value.has('queued')).toBe(true);
    expect(JSON.parse(localStorage.getItem('lucidos-collapsed-thread-queue-sections')!)).toEqual(['queued']);
  });
});
