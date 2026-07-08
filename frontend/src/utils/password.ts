/**
 * Shared password validation — single source of truth for the frontend.
 *
 * The backend (`crates/panel/src/service/password.rs::validate_password`)
 * enforces 8..=72 **UTF-8 bytes** (bcrypt's hard 72-byte truncation boundary).
 * Every password input in the UI MUST go through this util so the hints and the
 * client-side rejection can't drift from the server's rule (they previously did:
 * MainLayout/Account/Users-create used an antd `min: 6` *character* rule with no
 * upper bound, while Register/ForcePasswordChange/Users-reset used a copy-pasted
 * TextEncoder byte check — see v1.2).
 *
 * IMPORTANT: the check is BYTE length, not `.length` (UTF-16 code units). A
 * 6-char CJK / emoji password can be well over 8 bytes, and a 73-byte password
 * (which `.length` would happily report as e.g. 73 ASCII chars) is rejected by
 * bcrypt. TextEncoder.encode(...).length is the exact byte count the server
 * uses, so this matches `password.len()` in Rust.
 */

export type PasswordInvalidReason = 'tooShort' | 'tooLong';

export interface PasswordValidation {
  ok: boolean;
  /** Present only when `ok` is false. */
  reason?: PasswordInvalidReason;
}

const MIN_BYTES = 8;
const MAX_BYTES = 72;

/** Validate a password against the backend's 8..=72 UTF-8 byte rule. */
export function validatePassword(value: string): PasswordValidation {
  // An empty value is "ok" from this util's perspective — the antd `required`
  // rule handles "must not be empty". This lets the util be used as a field
  // validator without double-reporting "required" as "too short".
  if (value === '') return { ok: true };
  const bytes = new TextEncoder().encode(value).length;
  if (bytes < MIN_BYTES) return { ok: false, reason: 'tooShort' };
  if (bytes > MAX_BYTES) return { ok: false, reason: 'tooLong' };
  return { ok: true };
}

/** The minimum password length in UTF-8 bytes (matches the backend). */
export const PASSWORD_MIN_BYTES = MIN_BYTES;

/** The maximum password length in UTF-8 bytes (bcrypt's hard limit). */
export const PASSWORD_MAX_BYTES = MAX_BYTES;

/**
 * Build an antd-compatible form `validator` for a password field. Returns a
 * Promise that rejects with the appropriate i18n message when the password is
 * too short / too long. The `required` (empty) check is left to a separate antd
 * `required` rule (validatePassword treats '' as ok), so compose the field's
 * `rules` as `[{ required: true, message: ... }, { validator: makePasswordValidator(t('passwordTooShort'), t('passwordTooLong')) }]`.
 *
 * `tooShortMessage` / `tooLongMessage` are the pre-translated strings so this
 * util stays decoupled from the i18n layer's `t` type.
 */
export function makePasswordValidator(
  tooShortMessage: string,
  tooLongMessage: string,
): (rule: unknown, value: string) => Promise<void> {
  return (_rule, value) => {
    const result = validatePassword(value ?? '');
    if (result.ok) return Promise.resolve();
    return Promise.reject(
      new Error(result.reason === 'tooLong' ? tooLongMessage : tooShortMessage),
    );
  };
}
