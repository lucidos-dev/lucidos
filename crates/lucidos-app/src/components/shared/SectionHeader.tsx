import type { ComponentChildren, ComponentType } from 'preact';

/** The icon, label and count every list panel's section header draws: the
 *  thread drawer, Triggers, Changes and Thread queue. One markup, so the
 *  spacing and the count's look (styles/section-header.css) cannot differ
 *  between panels. A header with no count draws none. */
export function SectionHeaderContent({ Icon, title, count, running }: {
    Icon?: ComponentType<{ size?: string }>;
    title: ComponentChildren;
    count?: number | string;
    running?: boolean;
}) {
    return (
        <>
            {Icon && <span class="section-icon"><Icon size="0.875rem" /></span>}
            {/* While the section holds running work the label shimmers
                INVERTED: the bold label rests at full strength and a muted band
                sweeps across it. The standard dim base read as "dimmed" against
                this weight. */}
            <span class={`section-label${running ? ' running-shimmer running-shimmer-invert' : ''}`}>{title}</span>
            {count !== undefined && (
                <span class="section-count">
                    <span class="section-count-badge">{count}</span>
                    <span class="section-count-open">{count}</span>
                </span>
            )}
        </>
    );
}

/** A collapsible section header for a list panel. The toggle is a button, and
 *  the actions sit beside it rather than inside, so no button nests in another.
 *  The caller rolls the section body through `<Disclosure open={!collapsed}>`. */
export function SectionHeader({ title, count, collapsed, onToggle, actions, className }: {
    title: ComponentChildren;
    count?: number | string;
    collapsed: boolean;
    onToggle: () => void;
    actions?: ComponentChildren;
    className?: string;
}) {
    return (
        <div class={`list-section-title list-section-title-collapsible${collapsed ? ' collapsed' : ''}${className ? ` ${className}` : ''}`}>
            <button
                class="list-section-toggle"
                type="button"
                onClick={onToggle}
                aria-expanded={!collapsed}
            >
                <SectionHeaderContent title={title} count={count} />
            </button>
            {actions && <div class="list-section-actions">{actions}</div>}
        </div>
    );
}
