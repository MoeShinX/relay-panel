/* eslint-disable react-refresh/only-export-components */
import type { ReactNode } from 'react';
import { Tag, Tooltip, Typography } from 'antd';
import type { Tfn } from './types';
import type { NodeDisplayRow } from '../../api/types';
import { CountryFlag } from './CountryFlag';
import { formatPercent } from '../../utils/format';

const { Text } = Typography;

/** v1.2.13: "AMD EPYC 7B13 · 4 核" (node-v1.2.7+ reports both) — either half
 *  alone when only one is known, null when neither is (older nodes).
 *  `coresLabel` is the "{n} 核" template. */
export function cpuSummary(r: NodeDisplayRow, coresLabel: string): string | null {
  const parts = [
    r.cpu_model,
    r.cpu_cores ? coresLabel.replace('{n}', String(r.cpu_cores)) : null,
  ].filter(Boolean);
  return parts.length ? parts.join(' · ') : null;
}

/** The CPU bar's tooltip: usage, then the CPU model on a second line when the
 *  node reports one. */
export function cpuTooltip(r: NodeDisplayRow, t: Tfn): ReactNode {
  const model = cpuSummary(r, t('cpuCores'));
  return <>CPU: {formatPercent(r.cpu)}{model && <><br />{model}</>}</>;
}

/** Dual-stack network cell — IPv4 line + IPv6 line. Each line shows the
 *  CountryFlag pill (SVG, no Emoji) followed by the IP. No country name and
 *  no regionUnknown text: unknown regions render "--". */
export function NetworkCell({ row }: { row: NodeDisplayRow; t: Tfn }) {
  const v4 = row.public_ipv4 ?? row.public_ip;
  const v6 = row.public_ipv6;
  if (!v4 && !v6) return <Text type="secondary">-</Text>;
  const line = (ip: string, code: string | null | undefined) => (
    <div key={ip} style={{ fontSize: 12, lineHeight: '18px', display: 'flex', alignItems: 'center', gap: 6 }}>
      <CountryFlag code={code} />
      <span className="rp-mono" style={{ whiteSpace: 'nowrap' }}>{ip}</span>
    </div>
  );
  return (
    <>
      {v4 ? line(v4, row.ipv4_country_code) : null}
      {v6 ? line(v6, row.ipv6_country_code) : null}
    </>
  );
}

/** v1.2.12: an online node whose WS control channel is down. It forwards and
 *  reports fine over HTTP, so without this it looks healthy right up until an
 *  upgrade fails. Admin view only (`ws_connected` is absent elsewhere). */
export function wsDownTag(r: NodeDisplayRow, t: Tfn) {
  if (!r.online || r.ws_connected !== false) return null;
  return (
    <Tooltip title={t('nodeWsDownTip')}>
      <Tag color="orange">{t('nodeWsDown')}</Tag>
    </Tooltip>
  );
}

/** Status tag with protocol-mismatch detection. */
export function statusTag(r: NodeDisplayRow, t: Tfn, panelProtocol: number) {
  const v = r.config_protocol_version;
  if (v != null && panelProtocol > 0 && v !== panelProtocol) {
    return <Tag color="red">{t('protocolIncompatible')}</Tag>;
  }
  return r.online ? (
    <>
      <Tag color="green">{t('online')}</Tag>
      {wsDownTag(r, t)}
    </>
  ) : (
    <Tag>{t('offline')}</Tag>
  );
}
