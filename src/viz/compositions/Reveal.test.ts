// Reveal honesty: the cinematic's headline must never claim verification the
// pipeline didn't perform. A detector-only run (no brain key / brain failed)
// says so, in so many words.
import { describe, expect, it } from 'vitest';
import { revealHeading } from './Reveal';

describe('revealHeading', () => {
  it('labels a verified run', () => {
    expect(revealHeading(false)).toBe('VERIFIED DIAGNOSIS');
  });

  it('is honest about detector-only runs', () => {
    expect(revealHeading(true)).toBe('DETECTOR-ONLY DIAGNOSIS');
  });
});
