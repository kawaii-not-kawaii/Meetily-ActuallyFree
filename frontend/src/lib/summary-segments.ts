import type { Transcript } from '@/types';

/** Structured segment sent to `api_process_transcript` next to the flat text. Mirrors Rust `SummarySegment`. */
export interface SummarySegment {
  start_time?: number;
  end_time?: number;
  speaker?: string;
  text: string;
}

/**
 * `textOf` must be the same cleaner used for the flat transcript so segment text
 * equals the line bodies the model sees. Speaker is passed through verbatim.
 */
export function buildSummarySegments(
  transcripts: Transcript[],
  textOf: (t: Transcript) => string = t => t.text,
): SummarySegment[] {
  return transcripts.map(t => ({
    start_time: t.audio_start_time,
    end_time: t.audio_end_time,
    speaker: t.speaker || undefined,
    text: textOf(t),
  }));
}
