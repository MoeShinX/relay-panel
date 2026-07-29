import type { Dict } from '../i18n/zh-CN';

/**
 * v1.3.0: the display label for an announcement severity.
 *
 * The raw value (`info` / `success` / `warning` / `error`) is a wire value, not
 * something to show an operator. The tag labels are deliberately SHORT — the
 * colour-naming variants ("信息(蓝)") exist only in the form's picker, where
 * choosing a colour is the point; repeating it in a coloured tag is noise.
 *
 * Anything unrecognised falls back to the raw string rather than an empty tag,
 * so a value added on the backend before its label lands still reads.
 */
const LABEL_KEY: Record<string, keyof Dict> = {
  info: 'announcementKindInfo',
  success: 'announcementKindSuccess',
  warning: 'announcementKindWarning',
  error: 'announcementKindError',
};

export function kindLabel(t: (k: keyof Dict) => string, kind: string): string {
  const key = LABEL_KEY[kind];
  return key ? t(key) : kind;
}
