import { Badge } from './Badge';
import { STAGES, type Stage } from '../api/types';

/**
 * Deny-stage chip — renders ONLY the closed 10-value `Stage` set in pipeline
 * order (设计系统 §8). The component never fabricates a stage (notably never
 * `connect`): an unknown input renders a neutral "unknown" chip rather than
 * inventing a vocabulary value, keeping the basis the single source of truth.
 */

/** Ordinal position in the closed pipeline order (for ordering/semantics). */
export function stageOrder(stage: Stage): number {
  return STAGES.indexOf(stage);
}

function isStage(value: string): value is Stage {
  return (STAGES as readonly string[]).includes(value);
}

// Color temp roughly ascends along the pipeline; each is a fixed semantic.
const STAGE_CLASS: Record<Stage, string> = {
  auth: 'border-deny/40 text-deny',
  classify: 'border-warn/40 text-warn',
  rbac: 'border-deny/40 text-deny',
  constraint: 'border-warn/40 text-warn',
  condition: 'border-warn/40 text-warn',
  tier: 'border-info/40 text-info',
  transport: 'border-info/40 text-info',
  exec: 'border-cap-execute/40 text-cap-execute',
  audit: 'border-cap-manage/40 text-cap-manage',
  discover: 'border-text-muted/40 text-text-muted',
};

export function StageChip({ stage }: { stage: string }) {
  if (!isStage(stage)) {
    // Fail-closed display: surface an unknown marker, never a fabricated stage.
    return (
      <Badge className="border-text-muted/40 text-text-muted" title={`未知阶段: ${stage}`}>
        unknown
      </Badge>
    );
  }
  return (
    <Badge className={STAGE_CLASS[stage]} title={`阶段 ${stageOrder(stage) + 1}/10: ${stage}`}>
      {stage}
    </Badge>
  );
}
