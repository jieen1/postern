import { CapabilityBadge } from '../../../components';
import type { ResourceFormValues } from '../resourceForm';
import { declaresHighRisk } from '../resourceForm';

/**
 * Pre-submit summary preview (设计 §4.2 步骤3 / 统一写流程). Lists field-by-field
 * what will be written — notably that the real address becomes a
 * `vault://{code}/target` reference (plaintext not stored), the declared tiers +
 * verbs, and engine_enforced. High-risk verbs are flagged so the operator sees
 * the danger surface before confirming.
 */
export function SummaryPreview({
  values,
  editing,
}: {
  values: ResourceFormValues;
  editing: boolean;
}) {
  const highRisk = declaresHighRisk(values);
  const hasAddress = values.address.trim().length > 0;

  return (
    <div className="flex flex-col gap-3 text-sm">
      <p className="text-text-muted">
        将{editing ? '修订' : '接入'}资源{' '}
        <span className="font-mono text-text">{values.code}</span>，确认以下写入：
      </p>

      <dl className="grid grid-cols-[auto,1fr] gap-x-3 gap-y-1">
        <dt className="text-text-muted">adapter</dt>
        <dd className="font-mono">{values.adapter}</dd>
        <dt className="text-text-muted">transport</dt>
        <dd className="font-mono">{values.transport}</dd>
        <dt className="text-text-muted">engine_enforced</dt>
        <dd className="font-mono">{String(values.engine_enforced)}</dd>
        <dt className="text-text-muted">labels</dt>
        <dd className="font-mono">
          {values.labels.length === 0
            ? '—'
            : values.labels.map((l) => `${l.key}=${l.value}`).join(' · ')}
        </dd>
      </dl>

      <div>
        <div className="text-text-muted">真实地址</div>
        <p className="font-mono text-xs">
          {hasAddress
            ? `将转为 vault://${values.code}/target 引用，明文不入库`
            : '（未填写；现有引用保持不变）'}
        </p>
      </div>

      <div>
        <div className="mb-1 text-text-muted">声明的 tiers 及动词集</div>
        <ul className="flex flex-col gap-1">
          {values.tiers.map((t, i) => (
            <li key={`${t.tier}-${i}`} className="flex items-center gap-2">
              <span className="font-mono text-xs">{t.tier || '(未命名)'}</span>
              <span className="flex flex-wrap gap-1">
                {t.capabilities.map((c) => (
                  <CapabilityBadge key={c} capability={c} />
                ))}
              </span>
            </li>
          ))}
        </ul>
      </div>

      {highRisk.length > 0 && (
        <p role="alert" className="rounded-card border border-warn/40 bg-warn/5 px-3 py-2 text-xs text-warn">
          将声明高危动词面：{highRisk.join(' · ')} —— 提交需危险确认。
        </p>
      )}
    </div>
  );
}
