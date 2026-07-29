import { Table, Select, Space, Button, Tag, Tooltip, message, Typography } from 'antd';
import { ReloadOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, AuditEntry, AuditLogResponse } from '../api/types';
import { useI18n } from '../i18n/context';

const { Text } = Typography;

/**
 * The actions the backend records, in the order they appear in the filter.
 *
 * Deliberately a fixed list rather than something derived from the rows on
 * screen: a filter built from the current page would silently lose an option
 * as soon as that action scrolled off, which is exactly when you want to
 * filter for it.
 */
const ACTIONS = [
  'create_user',
  'delete_user',
  'reset_password',
  'reset_traffic',
  'admin_buy_plan',
  'admin_set_plan',
  'delete_rule',
  'restart_rule',
  'delete_group',
  'rotate_group_token',
  'upgrade_node',
  'create_redeem_codes',
  'void_redeem_code',
  'delete_redeem_codes',
  'redeem_code',
  'update_notify_settings',
  'update_site_settings',
  'create_announcement',
  'update_announcement',
  'delete_announcement',
] as const;

/** Actions that destroy something get a red tag, so a delete stands out when
 *  skimming a page of mostly routine entries. */
const DESTRUCTIVE = new Set<string>([
  'delete_user',
  'delete_rule',
  'delete_group',
  'delete_redeem_codes',
  'rotate_group_token',
  'reset_password',
  'reset_traffic',
  'void_redeem_code',
  'delete_announcement',
]);

const PAGE_SIZE = 20;

/**
 * v1.2.4: admin audit trail.
 *
 * Answers "who deleted my rule" from the panel instead of from a container log
 * that rotates and dies with the process.
 */
export default function AuditLog() {
  const { t } = useI18n();
  const [items, setItems] = useState<AuditEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [action, setAction] = useState<string>('');
  const [page, setPage] = useState(1);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const qs = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String((page - 1) * PAGE_SIZE),
      });
      if (action) qs.set('action', action);
      const res = await api.get<unknown, ApiEnvelope<AuditLogResponse>>(`/admin/audit-log?${qs}`);
      if (res.code !== 0) {
        message.error(res.message || t('loadFailed'));
        return;
      }
      setItems(res.data?.items ?? []);
      setTotal(res.data?.total ?? 0);
    } catch {
      message.error(t('loadFailed'));
    } finally {
      setLoading(false);
    }
  }, [action, page, t]);

  useEffect(() => { load(); }, [load]);

  /** Translate an action key, falling back to the raw key so an action added
   *  on the backend before its label lands still renders something readable. */
  const actionLabel = (key: string) => {
    const translated = t(`audit_${key}` as Parameters<typeof t>[0]);
    return translated === `audit_${key}` ? key : translated;
  };

  /** Translate a stored target type. Falls back to the raw value so a type
   *  added on the backend before its label lands still reads as something. */
  const targetTypeLabel = (type: string) => {
    // "settings" alone says nothing useful — which settings is the whole
    // point, and that lives in target_id.
    const key =
      type === 'settings'
        ? undefined
        : (`auditTarget${type.replace(/(^|_)(\w)/g, (_m, _s, c: string) => c.toUpperCase())}` as Parameters<typeof t>[0]);
    if (!key) return t('auditTargetSettings');
    const translated = t(key);
    return translated === key ? type : translated;
  };

  /** The whole target cell: a readable type plus whatever identifies the row. */
  const targetLabel = (type: string, id: string) => {
    if (!type) return '-';
    if (type === 'settings') {
      if (id === 'notify') return t('auditTargetSettingsNotify');
      if (id === 'site') return t('auditTargetSettingsSite');
      return t('auditTargetSettings');
    }
    return id ? `${targetTypeLabel(type)} ${id}` : targetTypeLabel(type);
  };

  const columns = [
    {
      title: t('auditTime'),
      dataIndex: 'ts',
      key: 'ts',
      width: 180,
      render: (ts: string) => <Text style={{ whiteSpace: 'nowrap' }}>{ts}</Text>,
    },
    {
      title: t('auditActor'),
      dataIndex: 'actor_name',
      key: 'actor_name',
      width: 140,
      // actor_name is a snapshot taken when the action happened, so it still
      // renders after the account is deleted — show the id alongside it.
      render: (name: string, row: AuditEntry) => (
        <Tooltip title={row.actor_id != null ? `ID ${row.actor_id}` : t('auditSystemActor')}>
          <span>{name || '-'}</span>
        </Tooltip>
      ),
    },
    {
      title: t('auditAction'),
      dataIndex: 'action',
      key: 'action',
      width: 160,
      render: (a: string) => (
        <Tag color={DESTRUCTIVE.has(a) ? 'red' : 'blue'}>{actionLabel(a)}</Tag>
      ),
    },
    {
      title: t('auditTarget'),
      key: 'target',
      width: 160,
      render: (_: unknown, row: AuditEntry) => targetLabel(row.target_type, row.target_id),
    },
    {
      title: t('auditDetail'),
      dataIndex: 'detail',
      key: 'detail',
      render: (d: string) => d || '-',
    },
  ];

  return (
    <div>
      <Space style={{ marginBottom: 16 }} wrap>
        <Select
          value={action}
          style={{ width: 220 }}
          onChange={(v) => { setAction(v); setPage(1); }}
          options={[
            { value: '', label: t('auditAllActions') },
            ...ACTIONS.map((a) => ({ value: a, label: actionLabel(a) })),
          ]}
        />
        <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
        <Text type="secondary">{t('auditRetentionHint')}</Text>
      </Space>
      <Table
        rowKey="id"
        loading={loading}
        columns={columns}
        dataSource={items}
        scroll={{ x: 'max-content' }}
        pagination={{
          current: page,
          pageSize: PAGE_SIZE,
          total,
          showSizeChanger: false,
          onChange: setPage,
        }}
      />
    </div>
  );
}
