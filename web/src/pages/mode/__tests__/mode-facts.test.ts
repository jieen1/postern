import { describe, expect, it } from 'vitest';
import {
  effectiveSource,
  freezeConfirmWord,
  scopeLabel,
  MODE_NARROWING,
} from '../mode-facts';

describe('effectiveSource — 取严来源标注（前端只标注，不另算取严）', () => {
  it('labels ←全局 when there is no local override row', () => {
    expect(effectiveSource(null, 'observe')).toBe('global');
  });

  it('labels ←本地 when the effective value equals the local value', () => {
    expect(effectiveSource('maintain', 'maintain')).toBe('local');
  });

  it('labels ←全局 when the global value out-strictens the local override', () => {
    // local=maintain but effective=freeze ⇒ global won the meet.
    expect(effectiveSource('maintain', 'freeze')).toBe('global');
  });
});

describe('freezeConfirmWord — freeze 防误触输入辖区标识（非字面 "freeze"）', () => {
  it('uses GLOBAL for the global jurisdiction (scope null)', () => {
    expect(freezeConfirmWord(null)).toBe('GLOBAL');
    expect(scopeLabel(null)).toBe('GLOBAL');
  });

  it('uses the resource code for a per-resource jurisdiction', () => {
    expect(freezeConfirmWord('db-main')).toBe('db-main');
    expect(scopeLabel('db-main')).toBe('db-main');
  });
});

describe('MODE_NARROWING — 客观事实陈述（不生成建议话术）', () => {
  it('freeze states it rejects ALL verbs including read-only', () => {
    expect(MODE_NARROWING.freeze).toMatch(/拒绝一切/);
    expect(MODE_NARROWING.freeze).toMatch(/含只读/);
  });

  it('observe admits only read verbs', () => {
    expect(MODE_NARROWING.observe).toMatch(/observe/);
    expect(MODE_NARROWING.observe).toMatch(/query/);
    expect(MODE_NARROWING.observe).not.toMatch(/mutate/);
  });

  it('normal narrows nothing', () => {
    expect(MODE_NARROWING.normal).toMatch(/放行全部/);
  });
});
