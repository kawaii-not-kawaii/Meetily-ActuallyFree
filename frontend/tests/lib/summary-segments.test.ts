import { describe, expect, test } from 'bun:test';
import { buildSummarySegments } from '../../src/lib/summary-segments';

const t = (o: object) => ({ id: '1', text: 'hello', timestamp: '10:00:00', ...o }) as any;

describe('buildSummarySegments', () => {
  test('keeps time, speaker label and text verbatim', () => {
    expect(buildSummarySegments([t({ audio_start_time: 1.5, audio_end_time: 3, speaker: 'Guest' })])).toEqual([
      { start_time: 1.5, end_time: 3, speaker: 'Guest', text: 'hello' },
    ]);
  });
  test('missing times/speaker stay undefined and cleaner text is used', () => {
    const [s] = buildSummarySegments([t({ speaker: '' })], x => x.text.toUpperCase());
    expect(s.start_time).toBeUndefined();
    expect(s.speaker).toBeUndefined();
    expect(s.text).toBe('HELLO');
  });
});
