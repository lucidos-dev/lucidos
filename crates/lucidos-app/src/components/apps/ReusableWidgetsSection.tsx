import { useEffect, useState } from 'preact/hooks';
import { appSearchQuery } from '../../store/store';
import { reusableWidgets } from '../../store/widgets';
import { loadReusableWidgets } from '../../store/actions/widgets';
import { openWidgetInCanvas, stopReusingWidget } from '../../store/actions/widget-actions';
import { SectionHeader } from '../shared/SectionHeader';
import { Disclosure } from '../shared/Disclosure';
import { LoadableError } from '../shared/LoadableError';
import { AppIcon } from '../shared/AppIcon';

/** The apps panel's "Widgets" group: the reusable widgets, which any thread
 *  may show (ADR 0402). A built-in one has no Stop reusing (ADR 0415). A tap opens one in Canvas. The group appears once
 *  there is one, below the apps list, which draws its own skeleton. */
export function ReusableWidgetsSection() {
  const [collapsed, setCollapsed] = useState(false);
  useEffect(() => { void loadReusableWidgets(); }, []);

  const loadable = reusableWidgets.value;
  if (loadable.status === 'failed') return <LoadableError noun="reusable widgets" error={loadable.error} />;
  if (loadable.status !== 'loaded') return null;
  const query = appSearchQuery.value.trim().toLowerCase();
  const widgets = query
    ? loadable.data.filter((w) => w.name.toLowerCase().includes(query) || w.description.toLowerCase().includes(query))
    : loadable.data;
  if (widgets.length === 0) return null;

  return (
    <div class="reusable-widgets" data-role="reusable-widgets">
      <SectionHeader
        title="Widgets"
        count={widgets.length}
        collapsed={collapsed}
        onToggle={() => setCollapsed(!collapsed)}
      />
      <Disclosure open={!collapsed}>
        <div class="list-rows">
          {widgets.map((widget) => (
            <div key={widget.id} class="list-row clickable" onClick={() => void openWidgetInCanvas(widget.id, widget.name)}>
              <div class="app-row-lead">
                <AppIcon appId={widget.id} name={widget.name} icon={widget.icon} />
                <div class="list-row-info">
                  <div class="title list-row-name">{widget.name}</div>
                  {widget.description && <div class="list-row-details">{widget.description}</div>}
                </div>
              </div>
              {widget.built_in ? (
                <span class="list-row-details">Built in</span>
              ) : (
                <div class="list-row-actions">
                  <button
                    class="action-btn action-btn-secondary"
                    onClick={(e) => { e.stopPropagation(); void stopReusingWidget(widget.id, widget.name); }}
                  >
                    Stop reusing
                  </button>
                </div>
              )}
            </div>
          ))}
        </div>
      </Disclosure>
    </div>
  );
}
