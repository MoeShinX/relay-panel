/* eslint-disable react-refresh/only-export-components */
 
import type { ReactNode } from 'react';
import { Progress, Tooltip, Typography } from 'antd';
import { formatPercent, formatBytes } from '../../utils/format';
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

/** Disk bar with a mount/used/total/percent tooltip. */
export function NodeDiskBar({
  usagePercent, used, total, mount, t,
}: {
  usagePercent?: number | null;
  used?: number | null;
  total?: number | null;
  mount?: string | null;
  t: Tfn;
}) {
  if (usagePercent == null && used == null) return <Text type="secondary">-</Text>;
  const pct = usagePercent ?? 0;
  const tip = (
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
