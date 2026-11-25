// @vitest-environment jsdom
import { describe, it, expect } from 'vitest';
import { render } from 'preact';
import type { ComponentType } from 'preact';
import { SectionHeaderContent } from '../shared/SectionHeader';
import { FolderIcon } from '../shared/icons';
import { SECTION_META } from './ThreadDrawer';

function drawnIconSize(Icon: ComponentType<{ size?: string }>) {
  const host = document.createElement('div');
  render(<SectionHeaderContent Icon={Icon} title="Section" />, host);
  const svg = host.querySelector('.section-icon svg')!;
  return [svg.getAttribute('width'), svg.getAttribute('height')];
}

describe('drawer section icons', () => {
  const icons: [string, ComponentType<{ size?: string }>][] = [
    ...Object.entries(SECTION_META).map(([name, meta]) => [name, meta.Icon] as [string, ComponentType<{ size?: string }>]),
    ['custom folder', FolderIcon],
  ];

  it.each(icons)('%s takes the header size, so Safari never draws it zero wide', (_name, Icon) => {
    const [width, height] = drawnIconSize(Icon);
    expect(width).toBeTruthy();
    expect(height).toBe(width);
  });
});
