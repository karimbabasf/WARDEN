// harnessColors.test.ts — single source of truth for harness colour/glyph/label.
import { describe, expect, it } from 'vitest';
import { HARNESS_COLORS, harnessColor } from './harnessColors';

describe('HARNESS_COLORS constant', () => {
  it('claude_code has the canonical vivid-orange hue', () => {
    expect(HARNESS_COLORS.claude_code.hue).toBe('#ff7a18');
  });

  it('codex has the canonical electric-blue hue', () => {
    expect(HARNESS_COLORS.codex.hue).toBe('#2e8bff');
  });

  it('unknown has the canonical slate hue', () => {
    expect(HARNESS_COLORS.unknown.hue).toBe('#8fa0b8');
  });

  it('each entry carries the canonical glyph', () => {
    expect(HARNESS_COLORS.claude_code.glyph).toBe('◆');
    expect(HARNESS_COLORS.codex.glyph).toBe('▣');
    expect(HARNESS_COLORS.unknown.glyph).toBe('◇');
  });

  it('each entry carries the canonical label', () => {
    expect(HARNESS_COLORS.claude_code.label).toBe('Claude');
    expect(HARNESS_COLORS.codex.label).toBe('Codex');
    expect(HARNESS_COLORS.unknown.label).toBe('Unknown');
  });

  it('ids are self-consistent', () => {
    expect(HARNESS_COLORS.claude_code.id).toBe('claude_code');
    expect(HARNESS_COLORS.codex.id).toBe('codex');
    expect(HARNESS_COLORS.unknown.id).toBe('unknown');
  });
});

describe('harnessColor()', () => {
  it('resolves claude_code to the orange hue', () => {
    expect(harnessColor('claude_code').hue).toBe('#ff7a18');
  });

  it('resolves codex glyph', () => {
    expect(harnessColor('codex').glyph).toBe('▣');
  });

  it('unknown fallback for an unrecognised harness id', () => {
    expect(harnessColor('weird').id).toBe('unknown');
  });

  it('null input falls back to unknown', () => {
    expect(harnessColor(null).label).toBe('Unknown');
  });

  it('undefined input falls back to unknown', () => {
    expect(harnessColor(undefined).id).toBe('unknown');
  });

  it('empty string falls back to unknown', () => {
    expect(harnessColor('').id).toBe('unknown');
  });

  it('is case-insensitive (CLAUDE_CODE → claude_code)', () => {
    expect(harnessColor('CLAUDE_CODE').hue).toBe('#ff7a18');
  });

  it('unknown fallback has the neutral slate colour (honest-viz)', () => {
    expect(harnessColor('gemini').hue).toBe('#8fa0b8');
  });

  it('unknown fallback colour is distinct from both brand hues', () => {
    const u = harnessColor('something_random');
    expect(u.hue).not.toBe('#ff7a18');
    expect(u.hue).not.toBe('#2e8bff');
  });
});
