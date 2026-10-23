import { describe, expect, it, vi } from 'vitest';
import { render, screen, within } from '@testing-library/react';
import type { NodeDisplayRow } from '../../api/types';
import { I18nContext } from '../../i18n/context';
import { zhCN } from '../../i18n/zh-CN';

// NodeDetailDrawer calls api.delete on the admin "delete status" action, so the
// client must be mocked before importing the component.
vi.mock('../../api/client', () => ({
  default: { delete: vi.fn().mockResolvedValue({ code: 0 }) },
}));

import { NodeDetailDrawer } from './NodeDetailDrawer';

const baseRow: NodeDisplayRow = {
  group_id: 1,
  group_name: 'g1',
  node_id: 'node-abc',
  online: true,
  cpu: 10,
  mem: 20,
  network_interface: 'eth0',
  config_protocol_version: 2,
};

describe('NodeDetailDrawer desensitization', () => {
  it('shows admin-only sensitive fields when isAdmin is true', () => {
    render(<NodeDetailDrawer row={baseRow} open onClose={vi.fn()} isAdmin={true} panelProtocol={2} />);
    // node_id value + admin-only labels are present
    expect(screen.getByText('node-abc')).toBeInTheDocument();
    expect(screen.getByText('configProtocolVersion')).toBeInTheDocument();
    expect(screen.getByText('networkInterface')).toBeInTheDocument();
    // delete-status action is admin-only
    expect(screen.getByText('nodeStatusDelete')).toBeInTheDocument();
  });

  it('hides node_id and all admin-only fields when isAdmin is false', () => {
    render(<NodeDetailDrawer row={baseRow} open onClose={vi.fn()} isAdmin={false} panelProtocol={2} />);
    // the raw node_id must never reach a regular user's DOM
    expect(screen.queryByText('node-abc')).not.toBeInTheDocument();
    expect(screen.queryByText('configProtocolVersion')).not.toBeInTheDocument();
    expect(screen.queryByText('networkInterface')).not.toBeInTheDocument();
    expect(screen.queryByText('nodeStatusDelete')).not.toBeInTheDocument();
    // safe metrics are still rendered (sanity: the drawer did open)
    expect(screen.getByText('nodeVersion')).toBeInTheDocument();
  });
});

// ── v1.2.13: the CPU model row (node-v1.2.7+ reports model + core count) ──
describe('NodeDetailDrawer CPU model', () => {
  // Real Chinese strings, so the "{n} 核" template is exercised too.
  const zh = { t: (k: keyof typeof zhCN) => zhCN[k], lang: 'zh-CN' as const, setLang: () => {} };
  const renderZh = (row: NodeDisplayRow, isAdmin = true) =>
    render(
      <I18nContext.Provider value={zh}>
        <NodeDetailDrawer row={row} open onClose={vi.fn()} isAdmin={isAdmin} panelProtocol={2} />
      </I18nContext.Provider>,
    );
  const cpuRow = () => screen.getByText('CPU 型号').closest('tr') as HTMLElement;

  it('shows the model and the core count', () => {
    renderZh({ ...baseRow, cpu_model: 'AMD EPYC 7B13', cpu_cores: 4 });
    expect(within(cpuRow()).getByText('AMD EPYC 7B13 · 4 核')).toBeInTheDocument();
  });

  it('shows whichever half is known', () => {
    renderZh({ ...baseRow, cpu_cores: 4 });
    expect(within(cpuRow()).getByText('4 核')).toBeInTheDocument();
  });

  it('shows a dash for an older node that reports neither', () => {
    renderZh(baseRow);
    expect(within(cpuRow()).getByText('-')).toBeInTheDocument();
  });

  it('adds no empty row to the user view', () => {
    renderZh(baseRow, false);
    expect(screen.queryByText('CPU 型号')).not.toBeInTheDocument();
  });

  // v1.2.14 (node-v1.2.8+): memory in bytes beside the percentage, as the
  // disk row has always shown; an older node keeps the percentage alone.
  it('shows memory used / total beside the percentage', () => {
    renderZh({ ...baseRow, mem: 12.5, mem_total: 8 * 1024 ** 3, mem_used: 1024 ** 3 });
    const memRow = screen.getByText('内存').closest('tr') as HTMLElement;
    expect(within(memRow).getByText('12.5% (1.00 GB / 8.00 GB)')).toBeInTheDocument();
  });

  it('keeps the percentage alone for an older node', () => {
    renderZh({ ...baseRow, mem: 12.5 });
    const memRow = screen.getByText('内存').closest('tr') as HTMLElement;
    expect(within(memRow).getByText('12.5%')).toBeInTheDocument();
  });
});

// The disk row lists every disk a node reports (nodes after node-v1.2.9); one
// disk, or an older node's none, keeps the single line it has always had.
describe('NodeDetailDrawer disks', () => {
  const zh = { t: (k: keyof typeof zhCN) => zhCN[k], lang: 'zh-CN' as const, setLang: () => {} };
  const GB = 1024 ** 3;
  const primary = { disk_usage_percent: 40, disk_used: 4 * GB, disk_total: 10 * GB, disk_mount: '/' };
  const root = { mount: '/', fs_type: 'ext4', total: 10 * GB, used: 4 * GB };
  const data = { mount: '/data', fs_type: 'xfs', total: 4 * GB, used: GB };
  const renderZh = (row: NodeDisplayRow) =>
    render(
      <I18nContext.Provider value={zh}>
        <NodeDetailDrawer row={row} open onClose={vi.fn()} isAdmin panelProtocol={2} />
      </I18nContext.Provider>,
    );
  const diskRow = () => screen.getByText('主硬盘').closest('tr') as HTMLElement;

  it('lists each disk', () => {
    renderZh({ ...baseRow, ...primary, disks: [root, data] });
    const r = diskRow();
    expect(within(r).getByText('/data')).toBeInTheDocument();
    expect(within(r).getByText(/xfs/)).toBeInTheDocument();
    expect(within(r).getByText('1.00 GB / 4.00 GB (25.0%)')).toBeInTheDocument();
    expect(within(r).getByText('4.00 GB / 10.00 GB (40.0%)')).toBeInTheDocument();
  });

  it('keeps the one-line summary for a single disk', () => {
    renderZh({ ...baseRow, ...primary, disks: [root] });
    expect(within(diskRow()).getByText('40.0% (4.00 GB/10.00 GB)')).toBeInTheDocument();
  });

  it('keeps the one-line summary for an older node', () => {
    renderZh({ ...baseRow, ...primary });
    expect(within(diskRow()).getByText('40.0% (4.00 GB/10.00 GB)')).toBeInTheDocument();
  });
});
