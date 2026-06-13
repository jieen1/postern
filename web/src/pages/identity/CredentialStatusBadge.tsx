import { Ban, CircleCheck, CircleSlash, Clock } from 'lucide-react';
import { Badge } from '../../components';
import {
  STATUS_LABEL,
  type CredentialStatus,
} from './schema';

/**
 * 凭证派生状态徽章（10-principals-credentials.md §五本页特有组件）。
 *
 * 复用基座语义色：生效=allow 绿、即将过期=warn 琥珀、已过期/已吊销=deny 红。
 * 状态不仅靠色：每个状态带固定图标 + 文案（§7 可访问性）。这是纯展示，不读
 * 任何机密字段。
 */
const STATUS_STYLE: Record<
  CredentialStatus,
  { cls: string; icon: typeof CircleCheck }
> = {
  active: { cls: 'border-allow/50 text-allow', icon: CircleCheck },
  near_expiry: { cls: 'border-warn/50 text-warn', icon: Clock },
  expired: { cls: 'border-deny/50 text-deny', icon: CircleSlash },
  revoked: { cls: 'border-deny/50 text-deny', icon: Ban },
};

export function CredentialStatusBadge({ status }: { status: CredentialStatus }) {
  const { cls, icon: Icon } = STATUS_STYLE[status];
  return (
    <Badge className={cls}>
      <Icon size={12} />
      {STATUS_LABEL[status]}
    </Badge>
  );
}
