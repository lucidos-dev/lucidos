/**
 * Content-pane category icon — the small glyph that labels a destination by
 * what KIND of content it is (a thread, file, app, trigger, settings page,
 * change, …). Single source of truth shared by Search Everywhere's result rows
 * and the content-pane back/forward history menu (PanelNav), so the same
 * category is marked the same way wherever it appears.
 *
 * Rendered as inline SVG (rather than pulled from icons.tsx) so the set stays
 * small and self-contained; every glyph strokes/fills with `currentColor` so a
 * caller tints the whole set via CSS.
 */
export function CategoryIcon({ category }: { category: string }) {
  const props = { width: '1rem', height: '1rem', viewBox: '0 0 16 16', fill: 'none', stroke: 'currentColor', 'stroke-width': '1.5', 'stroke-linecap': 'round' as const, 'stroke-linejoin': 'round' as const };
  switch (category) {
    case 'threads':
      return <svg {...props}><path d="M13 10a1.5 1.5 0 0 1-1.5 1.5H5.5L3 14V4.5A1.5 1.5 0 0 1 4.5 3h7A1.5 1.5 0 0 1 13 4.5z" /></svg>;
    case 'files':
      return <svg {...props}><path d="M9 2H5a1.5 1.5 0 0 0-1.5 1.5v9A1.5 1.5 0 0 0 5 14h6a1.5 1.5 0 0 0 1.5-1.5V6z" /><polyline points="9 2 9 6 12.5 6" /></svg>;
    case 'apps':
      return <svg {...props}><rect x="3" y="3" width="4" height="4" rx="0.5" /><rect x="9" y="3" width="4" height="4" rx="0.5" /><rect x="3" y="9" width="4" height="4" rx="0.5" /><rect x="9" y="9" width="4" height="4" rx="0.5" /></svg>;
    case 'plugins':
      return <svg {...props}><path d="M6 3.5a1.2 1.2 0 0 1 2.4 0c0 .5.4.8.9.8h1.7a.5.5 0 0 1 .5.5v1.7c0 .5.3.9.8.9a1.2 1.2 0 0 1 0 2.4c-.5 0-.8.4-.8.9v1.9a.5.5 0 0 1-.5.5H9.3c-.5 0-.9-.3-.9-.8a1.2 1.2 0 0 0-2.4 0c0 .5-.4.8-.9.8H3.5a.5.5 0 0 1-.5-.5V9.9c0-.5.3-.9.8-.9a1.2 1.2 0 0 0 0-2.4c-.5 0-.8-.4-.8-.9V4a.5.5 0 0 1 .5-.5h1.6c.5 0 .9-.3.9-.8z" /></svg>;
    case 'triggers':
      return <svg {...props}><circle cx="8" cy="8" r="5.5" /><path d="M8 4.5V8l2.5 1.5" /></svg>;
    case 'settings':
      return <svg {...props}><circle cx="8" cy="8" r="2" /><path d="M8 2.5v1M8 12.5v1M2.5 8h1M12.5 8h1M4.1 4.1l.7.7M11.2 11.2l.7.7M4.1 11.9l.7-.7M11.2 4.8l.7-.7" /></svg>;
    case 'changes':
      return <svg {...props}><path d="M4 3v10M12 3v4" /><circle cx="4" cy="13" r="1.5" fill="currentColor" stroke="none" /><circle cx="12" cy="7" r="1.5" fill="currentColor" stroke="none" /><path d="M12 8.5c0 2-2 3-4 4.5" /></svg>;
    case 'notifications':
      return <svg {...props}><path d="M12 6.5a4 4 0 0 0-8 0c0 4-1.5 5-1.5 5h11s-1.5-1-1.5-5Z" /><path d="M9.4 13.5a1.6 1.6 0 0 1-2.8 0" /></svg>;
    case 'web':
      return <svg {...props}><circle cx="8" cy="8" r="5.5" /><path d="M2.5 8h11" /><path d="M8 2.5a8.5 8.5 0 0 1 0 11 8.5 8.5 0 0 1 0-11Z" /></svg>;
    default:
      return <svg {...props}><circle cx="8" cy="8" r="5.5" /></svg>;
  }
}
