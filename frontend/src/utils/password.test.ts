import { describe, expect, it } from 'vitest';
import { validatePassword, PASSWORD_MIN_BYTES, PASSWORD_MAX_BYTES } from './password';

describe('validatePassword — byte boundaries (matches backend bcrypt rule)', () => {
  it('rejects a 7-byte (ASCII) password as too short', () => {
    expect(validatePassword('1234567')).toEqual({ ok: false, reason: 'tooShort' });
  });

  it('accepts an 8-byte (ASCII) password', () => {
    expect(validatePassword('12345678')).toEqual({ ok: true });
  });

  it('accepts a 72-byte (ASCII) password', () => {
    expect(validatePassword('a'.repeat(72))).toEqual({ ok: true });
  });

  it('rejects a 73-byte (ASCII) password as too long', () => {
    expect(validatePassword('a'.repeat(73))).toEqual({ ok: false, reason: 'tooLong' });
  });

  it('treats an empty string as ok (the antd `required` rule handles emptiness)', () => {
    expect(validatePassword('')).toEqual({ ok: true });
  });
});

describe('validatePassword — byte length, NOT character count', () => {
  // A 3-character CJK string is 9 UTF-8 bytes (3 bytes per CJK char) → passes
  // the 8-byte minimum even though `.length === 3`. This is the core reason the
  // old antd `min: 6` (character) rule was wrong.
  it('counts CJK characters by byte (3 chars = 9 bytes ≥ 8 → ok)', () => {
    expect(validatePassword('密码密码密')).toEqual({ ok: true });
    // 2 CJK chars = 6 bytes < 8 → too short (even though it's "2 characters").
    expect(validatePassword('密码')).toEqual({ ok: false, reason: 'tooShort' });
  });

  it('counts emoji by byte (a 4-byte emoji counts as 4 bytes)', () => {
    // '😀' is U+1F600 = 4 UTF-8 bytes. Two of them = 8 bytes → exactly ok.
    expect(validatePassword('😀😀')).toEqual({ ok: true });
    // One emoji = 4 bytes < 8 → too short.
    expect(validatePassword('😀')).toEqual({ ok: false, reason: 'tooShort' });
  });

  it('rejects a long multi-byte string that is short in characters but over 72 bytes', () => {
    // 25 CJK chars = 75 bytes > 72 → too long (`.length` would report 25).
    expect(validatePassword('密'.repeat(25))).toEqual({ ok: false, reason: 'tooLong' });
    // 24 CJK chars = 72 bytes → ok.
    expect(validatePassword('密'.repeat(24))).toEqual({ ok: true });
  });
});

describe('validatePassword — exported constants', () => {
  it('exposes the byte boundaries for hint text', () => {
    expect(PASSWORD_MIN_BYTES).toBe(8);
    expect(PASSWORD_MAX_BYTES).toBe(72);
  });
});
