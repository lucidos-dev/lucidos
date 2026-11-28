import type { ComponentChildren, ComponentType } from 'preact';
import { GlyphBadge } from './GlyphBadge';
import { ChevronRightIcon } from './icons';

export function SectionHeaderContent({ Icon, title, count, running, collapsible }: {
    Icon?: ComponentType<{ size?: string }>;
    title: ComponentChildren;
    count?: number | string;
    running?: boolean;
    collapsible?: boolean;
}) {
    return (
        <>
            {collapsible && <span class="section-chevron" aria-hidden="true"><ChevronRightIcon size="0.875rem" /></span>}
            {Icon && <span class="section-icon"><Icon size="0.875rem" /></span>}
            <span class={`section-label${running ? ' running-shimmer running-shimmer-invert' : ''}`}>{title}</span>
            {count !== undefined && (
                <span class="section-count">
                    <GlyphBadge class="section-count-badge">{count}</GlyphBadge>
                    <span class="section-count-open">{count}</span>
                </span>
            )}
        </>
    );
}

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
                <SectionHeaderContent title={title} count={count} collapsible />
            </button>
            {actions && <div class="list-section-actions">{actions}</div>}
        </div>
    );
}
