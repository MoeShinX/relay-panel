import { describe, expect, it } from 'vitest';
import { resolveNodeUpgrade } from './upgrade';
import type { NodeDisplayRow } from '../../api/types';

/** Build a node row with the fields resolveNodeUpgrade reads. */
function row(over: Partial<NodeDisplayRow>): NodeDisplayRow {
  return {
    group_id: 1, group_name: 'g', node_id: 'n1', online: true,
    node_version: '1.0.0', install_method: 'systemd', config_protocol_version: undefined,
    ...over,
  } as NodeDisplayRow;
}

describe('resolveNodeUpgrade — eligibility ladder (shared by desktop + mobile)', () => {
  // compareVersion = '1.1.0' throughout; node_version varies.

  it('returns "none" for a placeholder row (no node_id)', () => {
    expect(resolveNodeUpgrade(row({ node_id: null }), '1.1.0', 0, false).state).toBe('none');
  });

  it('returns "checkFailed" when the node-version lookup failed (priority, neutral)', () => {
    // Even though the node is behind, a failed lookup must NOT offer an upgrade.
    const r = row({ node_version: '1.0.0', install_method: 'systemd', online: true });
    expect(resolveNodeUpgrade(r, '', 0, true).state).toBe('checkFailed');
  });

  it('returns "protocolIncompatible" (priority over version)', () => {
    const r = row({ node_version: '1.1.0', config_protocol_version: 2 });
    expect(resolveNodeUpgrade(r, '1.1.0', 4, false).state).toBe('protocolIncompatible');
  });

  it('returns "unknown" when the node version is unparseable', () => {
    expect(resolveNodeUpgrade(row({ node_version: 'garbage' }), '1.1.0', 0, false).state).toBe('unknown');
  });

  it('returns "latest" when node == compareVersion (same)', () => {
    expect(resolveNodeUpgrade(row({ node_version: '1.1.0' }), '1.1.0', 0, false).state).toBe('latest');
  });

  it('returns "ahead" when node > compareVersion (leading build, never stale, never downgraded)', () => {
    expect(resolveNodeUpgrade(row({ node_version: '1.2.0' }), '1.1.0', 0, false).state).toBe('ahead');
  });

  it('returns "docker" when behind + docker install', () => {
    const r = row({ node_version: '1.0.0', install_method: 'docker' });
    expect(resolveNodeUpgrade(r, '1.1.0', 0, false).state).toBe('docker');
  });

  it('returns "manual" when behind + non-systemd install', () => {
    const r = row({ node_version: '1.0.0', install_method: 'manual' });
    expect(resolveNodeUpgrade(r, '1.1.0', 0, false).state).toBe('manual');
  });

  it('returns "upgradeable" when behind + systemd + online', () => {
    const r = row({ node_version: '1.0.0', install_method: 'systemd', online: true });
    expect(resolveNodeUpgrade(r, '1.1.0', 0, false).state).toBe('upgradeable');
  });

  it('returns "offline" when behind + systemd + offline', () => {
    const r = row({ node_version: '1.0.0', install_method: 'systemd', online: false });
    expect(resolveNodeUpgrade(r, '1.1.0', 0, false).state).toBe('offline');
  });

  it('panel-version-ahead does NOT make a current node look stale (compare target is the node release)', () => {
    // Panel is 1.2.0 but latest node release is 1.1.0; a node on 1.1.0 is
    // current (latest), NOT behind. This is the core PR5 scenario.
    expect(resolveNodeUpgrade(row({ node_version: '1.1.0' }), '1.1.0', 0, false).state).toBe('latest');
  });
});
