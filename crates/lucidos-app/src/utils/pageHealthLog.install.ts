/** Installs the page health counters. Imported near the top of `main.tsx`, so
 *  they count listeners that modules add while they load
 *  (`docs/temporary-measures.md` § Page health log). */
import { installPageHealthCounters } from './pageHealthLog';

installPageHealthCounters();
