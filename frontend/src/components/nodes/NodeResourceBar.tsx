/* eslint-disable react-refresh/only-export-components */
 
import type { ReactNode } from 'react';
import { Progress, Tooltip, Typography } from 'antd';
import { formatPercent, formatBytes } from '../../utils/format';
import type { NodeDisk } from '../../api/types';
import type { Tfn } from './types';

const { Text } = Typography;

/** Threshold color for a usage percent: <70 green, 70-89 orange, >=90 red. */
export function usageColor(p: number): string {
  if (p >= 90) return '#ff4d4f';
  if (p >= 70) return '#faad14';
  return '#52c41a';
}

/** A compact CPU/mem progress bar with a precise-value tooltip. Missing → "-"
 *  (never a misleading 0%). */
export function NodeResourceBar({ value, tooltip }: { value?: number | null; tooltip: ReactNode }) {
  if (value == null) return <Text type="secondary">-</Text>;
  const pct = Math.round(value);
  return (
    <Tooltip title={tooltip}>
      <Progress
        percent={pct}
        size="small"
        strokeColor={usageColor(value)}
        status={value >= 90 ? 'exception' : 'normal'}
        style={{ marginBottom: 0, minWidth: 60 }}
      />
    </Tooltip>
  );
}

/** Smallest disk shown besides `/`: 10 GiB. Kept in step with the node's
 *  `MIN_EXTRA_DISK_BYTES`; applied here as well because a node that has not been
 *  upgraded still reports every small partition and container volume. */
export const MIN_EXTRA_DISK_BYTES = 10 * 1024 * 1024 * 1024;

/** The disks worth listing: `/` always, any other only when it is a real disk's
 *  size (a 20 GB system disk and a 100 GB data disk, not a boot partition or a
 *  9.75 GB volume of a container host). */
export function visibleDisks(disks?: NodeDisk[] | null): NodeDisk[] {
  return (disks ?? []).filter((d) => d.mount === '/' || d.total >= MIN_EXTRA_DISK_BYTES);
}

/** More than one disk to show? A node with one (or an older node that lists
 *  none) keeps the single-disk display everywhere. */
export function hasDiskList(disks?: NodeDisk[] | null): disks is NodeDisk[] {
  return visibleDisks(disks).length > 1;
}

/** A disk's usage percent from its bytes; null without a total. */
export function diskUsagePercent(d: NodeDisk): number | null {
  return d.total > 0 ? (d.used / d.total) * 100 : null;
}

/** Every disk of a node, one group each: mount point and filesystem, then
 *  used / total and the usage percent. */
export function DiskList({ disks }: { disks: NodeDisk[] }) {
  return (
    <div style={{ fontSize: 12 }}>
      {visibleDisks(disks).map((d, i) => (
        <div key={`${d.mount}-${i}`} style={i > 0 ? { marginTop: 6 } : undefined}>
          <div style={{ wordBreak: 'break-all' }}>
            <span className="rp-mono">{d.mount}</span>
            <span style={{ opacity: 0.65 }}> · {d.fs_type}</span>
          </div>
          <div>
            {formatBytes(d.used)} / {formatBytes(d.total)} ({formatPercent(diskUsagePercent(d))})
          </div>
        </div>
      ))}
    </div>
  );
}

/** Disk bar with a mount/used/total/percent tooltip. The bar is always the
 *  primary disk; with several disks the tooltip lists them all. */
export function NodeDiskBar({
  usagePercent, used, total, mount, disks, t,
}: {
  usagePercent?: number | null;
  used?: number | null;
  total?: number | null;
  mount?: string | null;
  disks?: NodeDisk[] | null;
  t: Tfn;
}) {
  if (usagePercent == null && used == null) return <Text type="secondary">-</Text>;
  const pct = usagePercent ?? 0;
  const tip = hasDiskList(disks) ? (
    <DiskList disks={disks} />
  ) : (
    <div style={{ fontSize: 12 }}>
      <div>{t('diskMount')}: {mount || '-'}</div>
      <div>{t('diskUsed')}: {formatBytes(used)}</div>
      <div>{t('diskTotal')}: {formatBytes(total)}</div>
      <div>{t('diskUsage')}: {formatPercent(usagePercent)}</div>
    </div>
  );
  return (
    <Tooltip title={tip}>
      <Progress
        percent={Math.round(pct)}
        size="small"
        strokeColor={usageColor(pct)}
        status={pct >= 90 ? 'exception' : 'normal'}
        style={{ marginBottom: 0, minWidth: 60 }}
      />
    </Tooltip>
  );
}
