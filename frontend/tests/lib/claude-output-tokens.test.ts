import { describe, expect, test } from 'bun:test';
import { CLAUDE_FALLBACK_MODELS, parseClaudeOutputTokens, type ClaudeLimit } from '../../src/lib/claude-output-tokens';

const limit = (maxOutputTokens: number, source: ClaudeLimit['source'] = 'live'): ClaudeLimit =>
  ({ maxOutputTokens, maxInputTokens: null, source });

describe('Claude summary length cap', () => {
  test('blank means the model maximum', () => {
    expect(parseClaudeOutputTokens('  ', limit(128000))).toBeNull();
    expect(parseClaudeOutputTokens('', null)).toBeNull();
  });
  test('accepts caps up to the model limit, including 128K', () => {
    expect(parseClaudeOutputTokens('32000', limit(128000))).toBe(32000);
    expect(parseClaudeOutputTokens('128000', limit(128000))).toBe(128000);
  });
  test('rejects a cap above the limit instead of truncating', () => {
    expect(() => parseClaudeOutputTokens('64001', limit(64000))).toThrow('64,000');
    expect(() => parseClaudeOutputTokens('8193', limit(8192, 'unknown'))).toThrow('8,192');
  });
  test('rejects non-integers whether or not the limit has loaded', () => {
    for (const value of ['0', '-1', '1.5', '1e3', '12abc', 'NaN', '99999999999999999999']) {
      expect(() => parseClaudeOutputTokens(value, limit(64000))).toThrow();
      expect(() => parseClaudeOutputTokens(value, null)).toThrow();
    }
  });
  test('leaves the upper bound to the backend while the limit is not loaded', () => {
    expect(parseClaudeOutputTokens('100000', null)).toBe(100000);
  });
});

describe('Claude model catalog', () => {
  test('fallback list is the unified list without retired models', () => {
    expect(CLAUDE_FALLBACK_MODELS.map(m => m.id)).toEqual(['claude-sonnet-5-5', 'claude-opus-5-5', 'claude-haiku-5-5', 'claude-haiku-4-5']);
  });
});
