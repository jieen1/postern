import { Badge } from './Badge';
import type { Mode } from '../api/types';

/** Mode color mapping (设计系统 §3.1). */
const MODE_CLASS: Record<Mode, string> = {
  normal: 'border-mode-normal/50 text-mode-normal',
  observe: 'border-mode-observe/50 text-mode-observe',
  maintain: 'border-mode-maintain/50 text-mode-maintain',
  freeze: 'border-mode-freeze/60 text-mode-freeze',
};

export function ModeBadge({ mode }: { mode: Mode }) {
  return <Badge className={MODE_CLASS[mode]}>{mode}</Badge>;
}
