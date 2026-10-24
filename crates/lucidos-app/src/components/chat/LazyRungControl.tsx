import { lazyComponent } from '../../utils/lazyComponent';
import { pressControlMenu } from './controlMenuRequests';
import { RungButtons, type RungControlProps } from './RungButtons';

/** The control's boxes, drawn until its code arrives. A press on the trigger is
 *  held for the control, which takes it as it mounts (ADR 0436). */
function RungPlaceholder({ folded, rowAttrs }: RungControlProps) {
  if (folded) return null;
  return (
    <RungButtons
      shown={null}
      rowAttrs={rowAttrs}
      expanded={false}
      onOpen={(e) => pressControlMenu('rung', e.currentTarget as Element)}
    />
  );
}

/** Loads as the row first renders, off the first-paint set. */
export const LazyRungControl = lazyComponent(
  () => import('./RungControl').then((m) => m.RungControl),
  RungPlaceholder,
);
