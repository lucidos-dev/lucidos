import { useEffect } from 'preact/hooks';
import { installTooltips } from '@lucidos/tooltip';

export function useTooltip() {
  useEffect(() => installTooltips(), []);
}
