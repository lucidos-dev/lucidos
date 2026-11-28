import { preferences } from '../../store/store';
import { currentMemoryModule } from '../../store/actions/preferences';
import { Disclosure } from '../shared/Disclosure';
import { MemoryInspector } from './MemoryInspector';
import { MemoryModuleSection } from './MemoryModuleSection';
import { SummaryTreeBrowser } from './SummaryTreeBrowser';

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
