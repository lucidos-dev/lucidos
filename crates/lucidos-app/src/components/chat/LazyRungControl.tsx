import { lazyComponent } from '../../utils/lazyComponent';
import { pressControlMenu } from './controlMenuRequests';
import { RungButtons, type RungControlProps } from './RungButtons';

function RungPlaceholder({ folded, rowAttrs, disabledReason }: RungControlProps) {
  if (folded) return null;
  return (
    <RungButtons
      shown={null}
      disabledReason={disabledReason}
      rowAttrs={rowAttrs}
      expanded={false}
      onOpen={(e) => pressControlMenu('rung', e.currentTarget as Element)}
    />
  );
}

export const LazyRungControl = lazyComponent(
  () => import('./RungControl').then((m) => m.RungControl),
  RungPlaceholder,
);
