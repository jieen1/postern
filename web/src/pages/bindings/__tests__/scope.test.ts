import { describe, expect, it } from 'vitest';
import {
  buildResourceSpec,
  buildSelectorSpec,
  hasScopeContent,
  parseResourceSpec,
} from '../scope';

describe('buildSelectorSpec — what-you-see-is-what-you-send {all:[...]}', () => {
  it('builds an {all:[{key,value}]} object from rows', () => {
    const spec = buildSelectorSpec([
      { key: 'host', value: 'A' },
      { key: 'kind', value: 'docker' },
    ]);
    expect(JSON.parse(spec)).toEqual({
      all: [
        { key: 'host', value: 'A' },
        { key: 'kind', value: 'docker' },
      ],
    });
  });

  it('drops empty-value rows (a no-op label is never submitted)', () => {
    const spec = buildSelectorSpec([
      { key: 'host', value: '' },
      { key: 'env', value: 'prod' },
      { key: 'kind', value: '   ' },
    ]);
    expect(JSON.parse(spec)).toEqual({ all: [{ key: 'env', value: 'prod' }] });
  });

  it('empty rows ⇒ {all:[]} (empty set = fail-closed grant-nothing, 异常 B)', () => {
    expect(JSON.parse(buildSelectorSpec([]))).toEqual({ all: [] });
  });
});

describe('buildResourceSpec / parseResourceSpec', () => {
  it('joins codes and round-trips back', () => {
    expect(buildResourceSpec(['db-main', 'redis-main'])).toBe('db-main,redis-main');
    expect(parseResourceSpec('db-main,redis-main')).toEqual(['db-main', 'redis-main']);
  });

  it('trims and drops blanks on both directions', () => {
    expect(buildResourceSpec(['db-main', '', '  '])).toBe('db-main');
    expect(parseResourceSpec('db-main, , redis-main')).toEqual(['db-main', 'redis-main']);
  });
});

describe('hasScopeContent', () => {
  it('selector needs at least one non-empty row', () => {
    expect(hasScopeContent('selector', [{ key: 'host', value: '' }], [])).toBe(false);
    expect(hasScopeContent('selector', [{ key: 'host', value: 'A' }], [])).toBe(true);
  });
  it('resource needs at least one code', () => {
    expect(hasScopeContent('resource', [], [])).toBe(false);
    expect(hasScopeContent('resource', [], ['db-main'])).toBe(true);
  });
});
