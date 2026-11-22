import { describe, expect, it } from 'vitest';
import { usageColor } from './NodeResourceBar';

describe('usageColor', () => {
  it('is calm indigo below the 70% warning threshold', () => {
    expect(usageColor(0)).toBe('#6366f1');
    expect(usageColor(69)).toBe('#6366f1');
  });

  it('is amber in the 70–89% band', () => {
    expect(usageColor(70)).toBe('#d97706');
    expect(usageColor(89)).toBe('#d97706');
  });

  it('is rose red at or above 90%', () => {
    expect(usageColor(90)).toBe('#e11d48');
    expect(usageColor(100)).toBe('#e11d48');
  });
});
