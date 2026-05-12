import { SHORTCUTS, formatKey, type ShortcutBinding, type ShortcutCategory, type ShortcutDef } from '../../utils/shortcuts';

const CATEGORY_ORDER: ShortcutCategory[] = ['Navigation', 'View'];

function groupByCategory(): Map<ShortcutCategory, ShortcutDef[]> {
  const groups = new Map<ShortcutCategory, ShortcutDef[]>();
  for (const cat of CATEGORY_ORDER) groups.set(cat, []);
  for (const def of Object.values(SHORTCUTS)) {
    groups.get(def.category)!.push(def);
  }
  return groups;
}

function BindingDisplay({ binding }: { binding: ShortcutBinding }) {
  return (
    <span class="kbd-combo">
      {binding.keys.map((key, i) => (
        <kbd class="kbd-key" key={i}>{formatKey(key)}</kbd>
      ))}
    </span>
  );
}

function ShortcutRow({ def }: { def: ShortcutDef }) {
  return (
    <div class="settings-row shortcut-row">
      <div class="shortcut-row-info">
        <div class="shortcut-row-desc">{def.description}</div>
        {def.note && <div class="shortcut-row-note">{def.note}</div>}
      </div>
      <div class="shortcut-row-bindings">
        {def.bindings.map((binding, i) => (
          <span key={i} class="shortcut-row-binding">
            {i > 0 && <span class="shortcut-row-or">or</span>}
            <BindingDisplay binding={binding} />
          </span>
        ))}
      </div>
    </div>
  );
}

export function ShortcutsSection() {
  const groups = groupByCategory();
  return (
    <>
      {CATEGORY_ORDER.map((category) => (
        <div class="settings-section" key={category}>
          <div class="settings-section-title" data-search-anchor={`shortcuts:${category.toLowerCase()}`}>{category}</div>
          {groups.get(category)!.map((def, i) => (
            <ShortcutRow key={i} def={def} />
          ))}
        </div>
      ))}
    </>
  );
}
