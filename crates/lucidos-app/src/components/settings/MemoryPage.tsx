import { preferences } from '../../store/store';
import { currentMemoryModule } from '../../store/actions/preferences';
import { Disclosure } from '../shared/Disclosure';
import { MemoryInspector } from './MemoryInspector';
import { MemoryModuleSection } from './MemoryModuleSection';
import { SummaryTreeBrowser } from './SummaryTreeBrowser';

/** Settings → System → Memory. Each module's browser shows only while it is
 *  chosen: Classic's saved memories, or Tree's summary trees. Until
 *  preferences load the inspector shows, so a Classic workspace never sees it
 *  arrive late. */
export function MemoryPage() {
  const tree = preferences.value.status === 'loaded' && currentMemoryModule() === 'tree';
  return (
    <>
      <MemoryModuleSection />
      <Disclosure open={!tree}>
        <MemoryInspector />
      </Disclosure>
      <Disclosure open={tree}>
        <SummaryTreeBrowser />
      </Disclosure>
    </>
  );
}
