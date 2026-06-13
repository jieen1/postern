import { Badge } from './Badge';
import type { Capability } from '../api/types';

/**
 * Capability verb badge — fixed colors with danger ascending by color temp
 * (设计系统 §3.1). Read-only; never used as a decorative color.
 */
const CAP_CLASS: Record<Capability, string> = {
  observe: 'border-cap-observe/50 text-cap-observe',
  query: 'border-cap-query/50 text-cap-query',
  mutate: 'border-cap-mutate/50 text-cap-mutate',
  execute: 'border-cap-execute/50 text-cap-execute',
  manage: 'border-cap-manage/50 text-cap-manage',
  destroy: 'border-cap-destroy/50 text-cap-destroy',
};

export function CapabilityBadge({ capability }: { capability: Capability }) {
  return <Badge className={CAP_CLASS[capability]}>{capability}</Badge>;
}
