import { describe, expect, it, vi } from 'vitest';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { cpuSummary, memSummary, statusTag } from './shared';
import { NodeGroupSection } from './NodeGroupSection';
import { nodeDesktopColumnWidths } from './tableLayout';

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

describe('Node desktop table throughput layout', () => {
  it('prioritizes the upload/download rate column without widening the short status columns', () => {
    expect(nodeDesktopColumnWidths.rate).toBeGreaterThanOrEqual(192);
    expect(nodeDesktopColumnWidths.status).toBeLessThan(84);
    expect(nodeDesktopColumnWidths.nodeVersion).toBeLessThan(100);
    expect(nodeDesktopColumnWidths.nodeUpgrade).toBeLessThan(72);
  });

  it('keeps transfer values on one line', () => {
    const { container } = render(
      <NodeGroupSection
        rows={[row({ online: true, upload_bps: 1018430, download_bps: 60110, boot_upload_bytes: 123456789, boot_download_bytes: 987654321 })]}
        panelProtocol={0}
        latestNodeVersion="1.1.0"
        nodeVersionCheckFailed={false}
        isMobile={false}
        t={t}
        openDetail={vi.fn()}
      />,
    );

    expect(container.querySelectorAll('.rp-node-throughput')).toHaveLength(2);
  });
});

// ── v1.2.5: offline nodes are greyed so a stale row is distinguishable from a
// live one. The colours live in theme.css; what the component owes is putting
// the hook on the right rows and only those.
describe('NodeGroupSection offline styling', () => {
  it('greys an offline row and leaves an online one alone', () => {
    const rows = [
      row({ node_id: 'up', online: true, cpu: 10 }),
      row({ node_id: 'down', online: false, cpu: 90 }),
    ];
    const { container } = render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} />,
    );
    // .ant-table-row, not every tr: a horizontally-scrolling table also emits
    // a hidden measure row.
    const bodyRows = container.querySelectorAll('.ant-table-tbody > tr.ant-table-row');
    expect(bodyRows.length).toBe(2);
    expect(bodyRows[0].classList.contains('rp-node-offline')).toBe(false);
    expect(bodyRows[1].classList.contains('rp-node-offline')).toBe(true);
  });

  it('greys an offline card in the mobile list too', () => {
    const rows = [
      row({ node_id: 'up', online: true }),
      row({ node_id: 'down', online: false }),
    ];
    const { container } = render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={true} t={t} openDetail={vi.fn()} />,
    );
    expect(container.querySelectorAll('.rp-node-offline-card').length).toBe(1);
  });
});

// ── v1.2: node version is compared against the latest NODE release, not the
// panel version; protocol-incompatible takes priority; a failed node-version
// check shows a neutral state. These exercise the desktop upgrade column.
//
// The column is read-only: upgrading is done from the group header and the
// page header. A node that can be upgraded shows a marker (an img named by the
// "upgrade available" tip); nothing in the column is ever a button.
describe('NodeGroupSection v1.2 node-version comparison', () => {
  const marker = (v: string) => `nodeUpgradeTip`.replace('{v}', v);
  const renderRow = (over: Partial<NodeDisplayRow>, latest: string, opts: { checkFailed?: boolean; panelProtocol?: number } = {}) => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: undefined, ...over })];
    return render(
      <NodeGroupSection rows={rows} panelProtocol={opts.panelProtocol ?? 0} latestNodeVersion={latest} nodeVersionCheckFailed={opts.checkFailed ?? false} isMobile={false} t={t} openDetail={vi.fn()} onBatchUpgraded={vi.fn()} />,
    );
  };
  const upgradeButtons = () => screen.queryAllByRole('button', { name: 'nodeUpgrade' });

  // latest_node=1.1.0, node=1.1.0 → current: no marker. (A panel being ahead is
  // irrelevant — the comparison target is latest_node_version.)
  it('shows no upgrade marker when node == latest_node_version', () => {
    renderRow({}, '1.1.0');
    expect(screen.queryByRole('img', { name: marker('1.1.0') })).toBeNull();
    expect(upgradeButtons()).toHaveLength(0);
  });

  // latest_node=1.1.1, node=1.1.0 → behind: a marker, and still no per-node
  // button (the group / "upgrade all" buttons are how it gets upgraded).
  it('marks a node that is behind latest_node_version, without a per-node button', () => {
    renderRow({}, '1.1.1');
    expect(screen.getByRole('img', { name: marker('1.1.1') })).toBeInTheDocument();
    expect(upgradeButtons()).toHaveLength(0);
  });

  // node-version check failed → neutral state, no marker.
  it('shows a neutral state when the node version check failed', () => {
    renderRow({}, '', { checkFailed: true });
    expect(screen.queryByRole('img', { name: /nodeUpgradeTip/ })).toBeNull();
    expect(upgradeButtons()).toHaveLength(0);
  });

  // CONFIG_PROTOCOL_VERSION mismatch → "protocol incompatible" takes priority
  // over version status in the upgrade column, with no marker.
  it('flags protocol incompatibility over version status in the upgrade column', () => {
    renderRow({ config_protocol_version: 2 }, '1.1.0', { panelProtocol: 4 });
    expect(screen.getAllByText('protocolIncompatible').length).toBeGreaterThanOrEqual(1);
    expect(screen.queryByRole('img', { name: /nodeUpgradeTip/ })).toBeNull();
  });

  // node > latest_node → "leading version", treated as current, no marker.
  it('treats a node ahead of latest_node as current (no marker)', () => {
    renderRow({ node_version: '1.2.0' }, '1.1.0');
    expect(screen.queryByRole('img', { name: /nodeUpgradeTip/ })).toBeNull();
    expect(upgradeButtons()).toHaveLength(0);
  });

  // The column is the admin view: without the batch buttons it is not shown.
  it('hides the upgrade column from a regular user', () => {
    const rows = [row({ node_id: 'n1', online: true, node_version: '1.1.0', install_method: 'systemd', config_protocol_version: undefined })];
    render(
      <NodeGroupSection rows={rows} panelProtocol={0} latestNodeVersion="1.1.1" nodeVersionCheckFailed={false} isMobile={false} t={t} openDetail={vi.fn()} />,
    );
    expect(screen.queryByRole('img', { name: marker('1.1.1') })).toBeNull();
    expect(screen.queryByText('nodeUpgrade')).toBeNull();
  });
});

// ── v1.2.13: the CPU bar's tooltip names the CPU (node-v1.2.7+) ──
describe('CPU model in the CPU tooltip', () => {
  // Echo keys, except the core-count template, so "4 核" is really built.
  const tCores = ((key: string) => (key === 'cpuCores' ? '{n} 核' : key)) as unknown as Tfn;
  const section = (r: NodeDisplayRow, isMobile: boolean) =>
    render(
      <NodeGroupSection rows={[r]} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={isMobile} t={tCores} openDetail={vi.fn()} />,
    );

  it('summarises model and cores, or whichever is known', () => {
    expect(cpuSummary(row({ cpu_model: 'AMD EPYC 7B13', cpu_cores: 4 }), '{n} 核')).toBe('AMD EPYC 7B13 · 4 核');
    expect(cpuSummary(row({ cpu_model: 'AMD EPYC 7B13' }), '{n} 核')).toBe('AMD EPYC 7B13');
    expect(cpuSummary(row({ cpu_cores: 4 }), '{n} 核')).toBe('4 核');
    expect(cpuSummary(row({}), '{n} 核')).toBeNull();
  });

  for (const isMobile of [false, true]) {
    it(`shows the model under the usage on hover (${isMobile ? 'mobile' : 'desktop'})`, async () => {
      const { container } = section(
        row({ online: true, cpu: 3.2, cpu_model: 'AMD EPYC 7B13', cpu_cores: 4 }),
        isMobile,
      );
      await userEvent.setup().hover(container.querySelector('.ant-progress') as HTMLElement);
      const tip = await screen.findByText(/AMD EPYC 7B13 · 4 核/);
      expect(tip.textContent).toContain('CPU: 3.2%');
    });
  }

  it('shows only the usage for an older node', async () => {
    const { container } = section(row({ online: true, cpu: 3.2 }), false);
    await userEvent.setup().hover(container.querySelector('.ant-progress') as HTMLElement);
    const tip = await screen.findByText(/CPU: 3\.2%/);
    expect(tip.textContent).toBe('CPU: 3.2%');
  });
});

// v1.2.14: the memory tooltip said only "内存: 20.9%" — how much memory that
// is took a trip to the machine. node-v1.2.8 reports the bytes.
describe('memory in bytes in the memory tooltip', () => {
  const GB = 1024 ** 3;
  const section = (r: NodeDisplayRow, isMobile: boolean) =>
    render(
      <NodeGroupSection rows={[r]} panelProtocol={0} latestNodeVersion="1.1.0" nodeVersionCheckFailed={false} isMobile={isMobile} t={((k: string) => k) as unknown as Tfn} openDetail={vi.fn()} />,
    );
  const memBar = (c: HTMLElement) => c.querySelectorAll('.ant-progress')[1] as HTMLElement;

  it('summarises used / total, or nothing for an older node', () => {
    expect(memSummary(row({ mem_total: 8 * GB, mem_used: GB }))).toBe('1.00 GB / 8.00 GB');
    expect(memSummary(row({}))).toBeNull();
  });

  for (const isMobile of [false, true]) {
    it(`shows used / total under the usage on hover (${isMobile ? 'mobile' : 'desktop'})`, async () => {
      const { container } = section(
        row({ online: true, cpu: 3.2, mem: 12.5, mem_total: 8 * GB, mem_used: GB }),
        isMobile,
      );
      await userEvent.setup().hover(memBar(container));
      const tip = await screen.findByText(/1\.00 GB \/ 8\.00 GB/);
      expect(tip.textContent).toContain('mem: 12.5%');
    });
  }

  it('shows only the usage for an older node', async () => {
    const { container } = section(row({ online: true, cpu: 3.2, mem: 12.5 }), false);
    await userEvent.setup().hover(memBar(container));
    const tip = await screen.findByText(/mem: 12\.5%/);
    expect(tip.textContent).toBe('mem: 12.5%');
  });
});
