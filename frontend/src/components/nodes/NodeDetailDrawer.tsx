 
import { Drawer, Descriptions, Tag, Button, Popconfirm, message } from 'antd';
import { DeleteOutlined } from '@ant-design/icons';
import { formatPercent, formatBytes, formatBps, formatUptime } from '../../utils/format';
import { useI18n } from '../../i18n/context';
import { CountryFlag } from './CountryFlag';
import type { NodeDisplayRow } from '../../api/types';
import api from '../../api/client';

interface Props {
  row: NodeDisplayRow | null;
  open: boolean;
  onClose: () => void;
  isAdmin: boolean;
  panelProtocol: number;
  onDeleted?: () => void;
}

/** Full-metric detail drawer for one node. Admin sees extra fields (node_id,
 *  config protocol, interface, disk mount, process uptime, listener errors, a
 *  delete-status action). Regular users see only safe metrics — no node_id,
 *  no token, no config protocol, no listener errors, no management actions. */
export function NodeDetailDrawer({ row, open, onClose, isAdmin, panelProtocol, onDeleted }: Props) {
  const { t } = useI18n();
  const labels = { d: t('uptimeDay'), h: t('uptimeHour'), m: t('uptimeMinute'), s: t('uptimeSecond') };

  const handleDelete = async () => {
    if (!row) return;
    const gid = row.group_id;
    const nid = row.node_id;
    const url = nid
      ? `/nodes/${gid}?node_id=${encodeURIComponent(nid)}`
      : `/nodes/${gid}`;
    try {
      await api.delete(url);
      message.success(t('nodeStatusDeleted'));
      onDeleted?.();
      onClose();
    } catch {
      message.error(t('nodeStatusDeleteFailed'));
    }
  };

  const v4 = row?.public_ipv4 ?? row?.public_ip;
  const v6 = row?.public_ipv6;

  return (
    <Drawer title={row?.group_name || t('resourceDetails')} open={open} onClose={onClose} width={440}>
      {row && (
        <Descriptions column={1} size="small" bordered>
          <Descriptions.Item label={t('status')}>
            {row.online ? <Tag color="green">{t('online')}</Tag> : <Tag>{t('offline')}</Tag>}
          </Descriptions.Item>

          {/* Dual-stack network — flag pill + IP, no country name. Unknown
           *  regions render "--" inside the pill (see CountryFlag). */}
          {v4 && (
            <Descriptions.Item label="IPv4">
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <CountryFlag code={row.ipv4_country_code} />
                <span className="rp-mono">{v4}</span>
              </span>
            </Descriptions.Item>
          )}
          {v6 && (
            <Descriptions.Item label="IPv6">
              <span style={{ display: 'inline-flex', alignItems: 'center', gap: 6 }}>
                <CountryFlag code={row.ipv6_country_code} />
                <span className="rp-mono">{v6}</span>
              </span>
            </Descriptions.Item>
          )}

          <Descriptions.Item label={t('nodeVersion')}>{row.node_version || '-'}</Descriptions.Item>
          <Descriptions.Item label={t('connections')}>{row.connections || 0}</Descriptions.Item>
          <Descriptions.Item label="CPU">{formatPercent(row.cpu)}</Descriptions.Item>
          <Descriptions.Item label={t('mem')}>{formatPercent(row.mem)}</Descriptions.Item>
          <Descriptions.Item label={t('disk')}>
            {row.disk_usage_percent == null && row.disk_used == null
              ? '-'
              : `${formatPercent(row.disk_usage_percent)} (${formatBytes(row.disk_used)}/${formatBytes(row.disk_total)})`}
          </Descriptions.Item>
          <Descriptions.Item label={`${t('uploadRate')}/${t('downloadRate')}`}>
            {formatBps(row.upload_bps)} / {formatBps(row.download_bps)}
          </Descriptions.Item>
          <Descriptions.Item label={`${t('totalUpload')}/${t('totalDownload')}`}>
            {formatBytes(row.boot_upload_bytes)} / {formatBytes(row.boot_download_bytes)}
          </Descriptions.Item>
          <Descriptions.Item label={t('systemUptime')}>{formatUptime(row.uptime, labels)}</Descriptions.Item>
          <Descriptions.Item label={t('lastSeen')}>{row.last_seen || '-'}</Descriptions.Item>

          {/* Admin-only fields */}
          {isAdmin && (
            <>
              <Descriptions.Item label="node_id">{row.node_id || '-'}</Descriptions.Item>
              <Descriptions.Item label={t('configProtocolVersion')}>
                {(() => {
                  const v = row.config_protocol_version;
                  if (v == null) return '-';
                  if (panelProtocol > 0 && v !== panelProtocol) return <Tag color="red">v{v}</Tag>;
                  return <Tag color="green">v{v}</Tag>;
                })()}
              </Descriptions.Item>
              <Descriptions.Item label={t('networkInterface')}>{row.network_interface || '-'}</Descriptions.Item>
              <Descriptions.Item label={t('diskMount')}>{row.disk_mount || '-'}</Descriptions.Item>
              {row.process_uptime != null && (
                <Descriptions.Item label={t('processUptime')}>{formatUptime(row.process_uptime, labels)}</Descriptions.Item>
              )}
              <Descriptions.Item label={t('listeners')}>
                {(() => {
                  const errs = row.listener_errors;
                  if (!errs || errs.length === 0) return <Tag color="green">{t('ok')}</Tag>;
                  return <Tag color="red">{errs.length} {t('failed')}</Tag>;
                })()}
              </Descriptions.Item>
            </>
          )}
        </Descriptions>

      )}
      {isAdmin && row && (
        <div style={{ marginTop: 16 }}>
          <Popconfirm title={t('nodeStatusDeleteConfirm')} onConfirm={handleDelete}>
            <Button danger icon={<DeleteOutlined />}>{t('nodeStatusDelete')}</Button>
          </Popconfirm>
        </div>
      )}
    </Drawer>
  );
}
