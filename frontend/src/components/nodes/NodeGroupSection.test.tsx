import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen } from '@testing-library/react';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { statusTag } from './shared';
import { NodeGroupSection } from './NodeGroupSection';

// A fake t() that echoes the key — assertions match on the i18n KEY, not on a
// translated string, so the tests don't break when wording changes.
const t = ((key: string) => key) as unknown as Tfn;

function row(over: Partial<NodeDisplayRow>): NodeDisplayRow {
  return { group_id: 1, group_name: 'g1', node_id: 'n1', ...over };
}

describe('statusTag', () => {
  it('renders an online tag when the node is online', () => {
    render(<>{statusTag(row({ online: true }), t, 0)}</>);
    expect(screen.getByText('online')).toBeInTheDocument();
  });

  it('renders an offline tag when the node is offline', () => {
    render(<>{statusTag(row({ online: false }), t, 0)}</>);
    expect(screen.getByText('offline')).toBeInTheDocument();
  });

  it('flags a protocol mismatch over online/offline state', () => {
    // online is true, but the node's config protocol disagrees with the panel's
    render(<>{statusTag(row({ online: true, config_protocol_version: 1 }), t, 2)}</>);
    expect(screen.getByText('protocolIncompatible')).toBeInTheDocument();
    expect(screen.queryByText('online')).not.toBeInTheDocument();
  });
});

describe('NodeGroupSection mobile vs desktop', () => {
  // Disambiguate the two layouts: the desktop branch renders an antd Table
  // (.ant-table), the mobile branch renders plain card divs (no table).
  const rows = [row({ node_id: 'n1', online: true, cpu: 10 })];

  it('renders a table on desktop', () => {
    const { container } = render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} />,
    );
    expect(container.querySelector('.ant-table')).not.toBeNull();
  });

  it('renders no table (card list) on mobile', () => {
    const { container } = render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={true} t={t} openDetail={vi.fn()} />,
    );
    expect(container.querySelector('.ant-table')).toBeNull();
  });

  it('shows a "no node reporting" hint for a placeholder-only group', () => {
    const placeholder = [row({ node_id: null, online: false })];
    render(
      <NodeGroupSection rows={placeholder} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} />,
    );
    expect(screen.getByText('noNodeReportingInGroup')).toBeInTheDocument();
  });
});

// ── v1.2: node version is compared against the latest NODE release, not the
// panel version; protocol-incompatible takes priority; a failed node-version
// check shows a neutral state. These exercise the desktop upgrade column.
//
// Note on assertions: antd <Tooltip title> does not render its title as queryable
// DOM text in jsdom (it's portalled on hover), so these tests assert on the
// robust STRUCTURE instead — whether the clickable upgrade button is present
// (which only happens in the "behind + systemd + online" branch).
describe('NodeGroupSection v1.2 node-version comparison', () => {
  const onUpgrade = vi.fn();
  beforeEach(() => onUpgrade.mockReset());

  // Scenario 1 (task §IX.1): latest_node=1.1.0, node=1.1.0 → the node is
  // current (same as latest_node) → NO upgrade button. (A panel being ahead is
  // irrelevant — the comparison target is latest_node_version.)
  it('offers no upgrade button when node == latest_node_version', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: undefined })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={onUpgrade} />,
    );
    // No clickable upgrade button (the up-to-date state renders an icon, not a
    // Button with an onClick→onUpgrade). The only Buttons in the row are the
    // "resource details" link; clicking those never calls onUpgrade.
    expect(onUpgrade).not.toHaveBeenCalled();
  });

  // Scenario 2 (task §IX.2): latest_node=1.1.1, node=1.1.0 → behind → upgrade
  // button offered AND invokes onUpgrade when clicked (systemd + online).
  it('offers an upgrade (clickable) when node < latest_node_version (systemd online)', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: undefined })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.1" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={onUpgrade} />,
    );
    // The upgrade button is the one in the upgrade column whose onClick calls
    // onUpgrade(row). Find it by clicking every button and checking the mock.
    const buttons = screen.queryAllByRole('button');
    // Click the upgrade-column button (it's a type="link" small button with a
    // CloudDownloadOutlined icon; the details button is also type="link" but
    // calls openDetail). We assert at least one button triggers onUpgrade.
    let fired = false;
    for (const b of buttons) {
      const before = onUpgrade.mock.calls.length;
      b.click();
      if (onUpgrade.mock.calls.length > before) { fired = true; break; }
    }
    expect(fired).toBe(true);
  });

  // Scenario 4 (task §IX.4): node-version check failed → neutral state, NO
  // clickable upgrade button (clicking any button never fires onUpgrade).
  it('shows a neutral state (no upgrade trigger) when the node version check failed', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: undefined })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="" nodeVersionCheckFailed={true} isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={onUpgrade} />,
    );
    const buttons = screen.queryAllByRole('button');
    for (const b of buttons) b.click();
    expect(onUpgrade).not.toHaveBeenCalled();
  });

  // Scenario 6 (task §IX.6): CONFIG_PROTOCOL_VERSION mismatch → "protocol
  // incompatible" takes priority over version status in the upgrade column.
  // The node is current (1.1.0 == latest_node 1.1.0) AND systemd online — which
  // would normally show an up-to-date check — but the protocol mismatch must
  // surface the red "protocolIncompatible" tag instead.
  it('flags protocol incompatibility over version status in the upgrade column', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: 2 })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={4} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={onUpgrade} />,
    );
    // The protocol-incompatible tag renders in BOTH the status column and the
    // upgrade column, so there are at least 2 occurrences.
    expect(screen.getAllByText('protocolIncompatible').length).toBeGreaterThanOrEqual(1);
    // And no upgrade trigger fires (priority over the up-to-date/upgrade path).
    const buttons = screen.queryAllByRole('button');
    for (const b of buttons) b.click();
    expect(onUpgrade).not.toHaveBeenCalled();
  });

  // Scenario 5 (task §IX.5): node > latest_node → "leading version", no
  // downgrade, treated as current (no upgrade trigger).
  it('treats a node ahead of latest_node as current (no downgrade trigger)', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.2.0', install_method: 'systemd', config_protocol_version: undefined })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} onUpgrade={onUpgrade} />,
    );
    const buttons = screen.queryAllByRole('button');
    for (const b of buttons) b.click();
    expect(onUpgrade).not.toHaveBeenCalled();
  });
});
