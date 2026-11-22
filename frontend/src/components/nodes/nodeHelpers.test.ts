import { describe, expect, it } from 'vitest';
import { usageColor } from './NodeResourceBar';

describe('usageColor', () => {
  it('is green below the 70% warning threshold', () => {
    expect(usageColor(0)).toBe('#52c41a');
    expect(usageColor(69)).toBe('#52c41a');
  });

  it('is orange in the 70–89% band', () => {
    expect(usageColor(70)).toBe('#faad14');
    expect(usageColor(89)).toBe('#faad14');
  });

  it('is red at or above 90%', () => {
    expect(usageColor(90)).toBe('#ff4d4f');
    expect(usageColor(100)).toBe('#ff4d4f');
  });
});
