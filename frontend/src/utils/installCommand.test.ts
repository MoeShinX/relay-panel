import { describe, expect, it } from 'vitest';
import { INSTALL_SCRIPT_URL, INSTALL_TRUSTED_PREFIXES, buildInstallCommand } from './installCommand';

describe('buildInstallCommand', () => {
  const cmd = buildInstallCommand('tok-123', 'https://panel.example.com');

  it('is a single line', () => {
    expect(cmd).not.toContain('\n');
  });

  it('tries direct, then the trusted relay, and nothing else', () => {
    expect(INSTALL_TRUSTED_PREFIXES).toEqual(['https://down.relaypanel.dev/']);
    expect(cmd).toContain("for p in '' https://down.relaypanel.dev/; do");
    // The installer is a script run as root and cannot be checked against a
    // hash, so no public third-party accelerator may ever supply it.
    expect(cmd).not.toContain('gh-proxy.com');
    expect(cmd).not.toContain('ghfast.top');
    // The full GitHub URL is appended after the prefix, once, not repeated per source.
    expect(cmd.split(INSTALL_SCRIPT_URL).length - 1).toBe(1);
    expect(cmd).toContain(`"\${p}${INSTALL_SCRIPT_URL}"`);
  });

  it('gives each source 5 s to connect, 20 s in all and 2 MB at most', () => {
    expect(cmd).toContain(
      'curl -fsSL --connect-timeout 5 --max-time 20 --max-filesize 2097152 -o "$f"',
    );
  });

  it('stops at the first source that works and only then runs the installer', () => {
    expect(cmd).toContain('&& break; done && bash "$f" -t tok-123 -u https://panel.example.com;');
    expect(cmd.endsWith('rm -f "$f"')).toBe(true);
  });

  it('does not pipe a possibly truncated download straight into bash', () => {
    expect(cmd).not.toContain('<(');
    expect(cmd).not.toMatch(/curl[^;]*\|\s*bash/);
  });
});
