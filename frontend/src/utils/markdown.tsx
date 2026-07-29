import type { ReactNode } from 'react';

/**
 * v1.3.0: a deliberately tiny Markdown subset for the site announcement.
 *
 * Renders to **React elements, never to an HTML string**. That is the whole
 * design: React escapes text nodes, so there is no sanitizer to get wrong and
 * no `dangerouslySetInnerHTML` anywhere in this repo. A stored-markup field
 * that reaches every signed-in user's page is exactly where a compromised admin
 * account would want an injection point, and the announcement is the first
 * thing the panel renders from operator-authored input.
 *
 * Supported, and nothing else:
 *   `**bold**`  `*italic*`  `` `code` ``  `[text](https://…)`  `- list item`
 *
 * Anything unrecognised stays literal text, which is the right failure mode:
 * an operator who types an asterisk sees an asterisk.
 */

/** Only these schemes become links. Everything else — `javascript:`, `data:`,
 *  protocol-relative `//evil` — renders as the literal source text. */
function isSafeHref(url: string): boolean {
  return /^https?:\/\//i.test(url.trim());
}

interface Rule {
  re: RegExp;
  render: (m: RegExpExecArray, key: string) => ReactNode;
}

// Order matters: code first so `**` inside backticks stays literal, and bold
// before italic so `**x**` is not read as an italic `*` around `*x*`.
const RULES: Rule[] = [
  {
    re: /`([^`\n]+)`/,
    render: (m, key) => (
      <code key={key} className="rp-mono" style={{ padding: '0 4px' }}>{m[1]}</code>
    ),
  },
  {
    re: /\[([^\]\n]+)\]\(([^)\s]+)\)/,
    render: (m, key) =>
      isSafeHref(m[2]) ? (
        // noopener/noreferrer: the announcement links somewhere the operator
        // chose, but the target still must not get a handle on this window.
        <a key={key} href={m[2]} target="_blank" rel="noopener noreferrer">{m[1]}</a>
      ) : (
        // Not a link — show exactly what was typed, so a rejected scheme is
        // visible to the operator rather than silently swallowed.
        <span key={key}>{m[0]}</span>
      ),
  },
  {
    re: /\*\*([^*\n]+)\*\*/,
    render: (m, key) => <strong key={key}>{m[1]}</strong>,
  },
  {
    re: /\*([^*\n]+)\*/,
    render: (m, key) => <em key={key}>{m[1]}</em>,
  },
];

/**
 * Flatten the markdown subset to plain text, for the account-page summary bar.
 *
 * The banner is a prompt, not a reading surface: a two-line excerpt with half a
 * bold run and a dangling list dash reads worse than the same words plain. The
 * formatting is rendered in full on the archive page.
 *
 * Mirrors the RULES above deliberately — anything the parser treats as markup
 * has to be stripped here too, or the marker characters leak into the summary.
 */
export function stripMarkdown(src: string): string {
  return (
    src
      // Code first, so ** inside a code span is not read as bold.
      .replace(/`([^`\n]+)`/g, '$1')
      // Links keep their label; the URL is noise in a one-line summary.
      .replace(/\[([^\]\n]+)\]\(([^)\s]+)\)/g, '$1')
      // Bold before italic, so **x** does not degrade to *x*.
      .replace(/\*\*([^*\n]+)\*\*/g, '$1')
      .replace(/\*([^*\n]+)\*/g, '$1')
      // List markers only at the start of a line — a hyphen mid-sentence stays.
      .replace(/^\s*-\s+/gm, '')
      // Blank lines and newlines collapse: the summary is one flowing run.
      .replace(/\s*\n+\s*/g, ' ')
      .replace(/\s{2,}/g, ' ')
      .trim()
  );
}

/** Parse one line's inline markup into React nodes. */
export function renderInline(line: string, keyPrefix = ''): ReactNode[] {
  const out: ReactNode[] = [];
  let rest = line;
  let i = 0;

  while (rest.length > 0) {
    // Find the rule matching earliest in what's left; ties go to the rule
    // listed first, which is what gives code and bold their precedence.
    let best: { rule: Rule; m: RegExpExecArray } | null = null;
    for (const rule of RULES) {
      const m = rule.re.exec(rest);
      if (m && (best === null || m.index < best.m.index)) best = { rule, m };
    }
    if (!best) {
      out.push(rest);
      break;
    }
    if (best.m.index > 0) out.push(rest.slice(0, best.m.index));
    out.push(best.rule.render(best.m, `${keyPrefix}i${i++}`));
    rest = rest.slice(best.m.index + best.m[0].length);
  }

  return out;
}

/**
 * Parse the block structure: consecutive `- ` lines become one list, everything
 * else is a paragraph line. Blank lines are kept as spacing rather than
 * collapsed, because operators write announcements with deliberate gaps.
 */
export function renderMarkdown(src: string): ReactNode {
  const lines = src.split('\n');
  const blocks: ReactNode[] = [];
  let bullets: string[] = [];

  const flushBullets = () => {
    if (bullets.length === 0) return;
    const items = bullets;
    bullets = [];
    blocks.push(
      <ul key={`ul${blocks.length}`} style={{ margin: '4px 0', paddingLeft: 20 }}>
        {items.map((b, n) => (
          <li key={n}>{renderInline(b, `l${blocks.length}_${n}_`)}</li>
        ))}
      </ul>,
    );
  };

  lines.forEach((line, n) => {
    const bullet = /^\s*-\s+(.*)$/.exec(line);
    if (bullet) {
      bullets.push(bullet[1]);
      return;
    }
    flushBullets();
    blocks.push(
      <div key={`p${n}`} style={{ minHeight: line.trim() === '' ? '0.6em' : undefined }}>
        {renderInline(line, `p${n}_`)}
      </div>,
    );
  });
  flushBullets();

  return <>{blocks}</>;
}
