import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import { renderMarkdown, renderInline, stripMarkdown } from './markdown';

const show = (src: string) => render(<div data-testid="out">{renderMarkdown(src)}</div>);
const out = () => screen.getByTestId('out');

describe('renderInline', () => {
  it('renders bold, italic and code', () => {
    render(<div data-testid="out">{renderInline('a **b** c *d* e `f`')}</div>);
    expect(out().querySelector('strong')?.textContent).toBe('b');
    expect(out().querySelector('em')?.textContent).toBe('d');
    expect(out().querySelector('code')?.textContent).toBe('f');
  });

  it('does not parse markup inside code spans', () => {
    // Code wins because it is first in the rule list; without that, `**` inside
    // a code sample would silently turn into bold.
    render(<div data-testid="out">{renderInline('`**not bold**`')}</div>);
    expect(out().querySelector('strong')).toBeNull();
    expect(out().querySelector('code')?.textContent).toBe('**not bold**');
  });

  it('reads ** as bold, not as italic around an asterisk', () => {
    render(<div data-testid="out">{renderInline('**x**')}</div>);
    expect(out().querySelector('strong')?.textContent).toBe('x');
    expect(out().querySelector('em')).toBeNull();
  });

  it('leaves unmatched markers as literal text', () => {
    // An operator typing a bare asterisk should see a bare asterisk.
    render(<div data-testid="out">{renderInline('2 * 3 = 6')}</div>);
    expect(out().textContent).toBe('2 * 3 = 6');
  });
});

describe('renderMarkdown links', () => {
  it('links http and https', () => {
    show('see [docs](https://example.com/x)');
    const a = out().querySelector('a');
    expect(a?.getAttribute('href')).toBe('https://example.com/x');
    expect(a?.getAttribute('rel')).toContain('noopener');
  });

  it.each([
    'javascript:alert(1)',
    'JaVaScRiPt:alert(1)',
    'data:text/html,<script>alert(1)</script>',
    '//evil.example.com',
    'vbscript:msgbox',
  ])('refuses to link %s', (url) => {
    show(`click [here](${url})`);
    // No anchor at all, and the raw source stays visible so the operator can
    // see their link was not accepted.
    expect(out().querySelector('a')).toBeNull();
    expect(out().textContent).toContain(url);
  });
});

describe('renderMarkdown blocks', () => {
  it('groups consecutive dash lines into one list', () => {
    show('note:\n- one\n- two\nafter');
    const lis = out().querySelectorAll('li');
    expect(Array.from(lis).map((e) => e.textContent)).toEqual(['one', 'two']);
    expect(out().querySelectorAll('ul').length).toBe(1);
  });

  it('parses inline markup inside list items', () => {
    show('- **hi**');
    expect(out().querySelector('li strong')?.textContent).toBe('hi');
  });

  it('keeps line breaks as separate blocks', () => {
    show('line one\nline two');
    expect(out().textContent).toContain('line one');
    expect(out().textContent).toContain('line two');
  });
});

describe('renderMarkdown never emits raw HTML', () => {
  it.each([
    '<img src=x onerror=alert(1)>',
    '<script>alert(1)</script>',
    '<b>bold?</b>',
    '<iframe src="https://evil.example.com"></iframe>',
  ])('renders %s as visible text', (payload) => {
    show(payload);
    // React escapes text nodes, so the tag is displayed, not constructed.
    // This is the property that makes the whole feature safe — assert the
    // element does not exist AND the source is on screen.
    expect(out().textContent).toContain(payload);
    expect(out().querySelector('img,script,iframe,b')).toBeNull();
  });

  it('escapes markup that arrives inside a link label', () => {
    show('[<img src=x onerror=alert(1)>](https://example.com)');
    expect(out().querySelector('img')).toBeNull();
    expect(out().querySelector('a')?.textContent).toContain('<img');
  });
});

describe('stripMarkdown', () => {
  it('removes every marker the parser treats as markup', () => {
    // If the two ever disagree, the marker characters leak into the summary
    // bar — which is the one place they must not appear.
    expect(stripMarkdown('**粗** 和 *斜* 和 `码`')).toBe('粗 和 斜 和 码');
  });

  it('keeps a link label and drops the URL', () => {
    expect(stripMarkdown('详见 [公告页](https://example.com/x)')).toBe('详见 公告页');
  });

  it('flattens list markers and blank lines into one run', () => {
    const src = ['维护通知', '', '- 香港不受影响', '- 日本重启', '', '结束'].join('\n');
    expect(stripMarkdown(src)).toBe('维护通知 香港不受影响 日本重启 结束');
  });

  it('leaves a mid-sentence hyphen and a lone asterisk alone', () => {
    // Only line-leading "- " is a list marker, and a single asterisk is not
    // emphasis — the summary should read like what the operator typed.
    expect(stripMarkdown('02:00 - 04:00 维护,2 * 3 = 6')).toBe('02:00 - 04:00 维护,2 * 3 = 6');
  });

  it('does not turn bold into a stray italic marker', () => {
    // Stripping italic first would leave "*x*" behind.
    expect(stripMarkdown('**x**')).toBe('x');
  });

  it('leaves unsupported syntax as typed', () => {
    expect(stripMarkdown('# 不是标题')).toBe('# 不是标题');
    expect(stripMarkdown('~~不是删除线~~')).toBe('~~不是删除线~~');
  });

  it('does not execute or unwrap HTML', () => {
    // The summary is rendered as a React text node, so this stays literal —
    // asserted here so a future "smarter" stripper cannot quietly unwrap it.
    expect(stripMarkdown('<b>x</b>')).toBe('<b>x</b>');
  });
});
