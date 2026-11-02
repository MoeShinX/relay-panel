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
  const data = { mount: '/data', fs_type: 'xfs', total: 100 * GB, used: 25 * GB };
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
    expect(within(r).getByText('25.00 GB / 100.00 GB (25.0%)')).toBeInTheDocument();
    expect(within(r).getByText('4.00 GB / 10.00 GB (40.0%)')).toBeInTheDocument();
  });

  // The list is of disks, not of every partition and volume a machine mounts:
  // `/` always, any other one only from 10 GiB up.
  it('leaves out small partitions and container volumes, from nodes that still report them', () => {
    const small = [
      { mount: '/recovery', fs_type: 'ext4', total: GB / 2, used: 100 },
      { mount: '/srv/volumes/tenant-4', fs_type: 'ext4', total: 9.75 * GB, used: 0.04 * GB },
      { mount: '/srv/volumes/tenant-5', fs_type: 'ext4', total: 9.75 * GB, used: 0.07 * GB },
    ];
    renderZh({ ...baseRow, ...primary, disks: [root, ...small, data] });
    const r = diskRow();
    expect(within(r).getByText('/data')).toBeInTheDocument();
    expect(within(r).queryByText('/recovery')).toBeNull();
    expect(within(r).queryByText(/tenant/)).toBeNull();
  });

  it('is the one-line summary when only small partitions are left besides /', () => {
    const small = { mount: '/boot2', fs_type: 'ext4', total: 2 * GB, used: GB };
    renderZh({ ...baseRow, ...primary, disks: [root, small] });
    expect(within(diskRow()).getByText('40.0% (4.00 GB/10.00 GB)')).toBeInTheDocument();
    expect(within(diskRow()).queryByText('/boot2')).toBeNull();
  });

  it('shows a disk exactly 10 GiB big, and a small / still shows', () => {
    const edge = { mount: '/hdd', fs_type: 'ext4', total: 10 * GB, used: GB };
    renderZh({ ...baseRow, ...primary, disks: [{ ...root, total: 2 * GB, used: GB }, edge] });
    const r = diskRow();
    expect(within(r).getByText('/hdd')).toBeInTheDocument();
    expect(within(r).getByText('/')).toBeInTheDocument();
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
