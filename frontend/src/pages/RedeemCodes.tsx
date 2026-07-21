import { Table, Button, Modal, Form, InputNumber, Input, Select, Space, message, Popconfirm, Tag, Typography, Alert } from 'antd';
import { PlusOutlined, ReloadOutlined, CopyOutlined, DownloadOutlined, DeleteOutlined, StopOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, RedeemCode, ListCodesResponse, CreateCodesResponse } from '../api/types';
import { MAX_REDEEM_BATCH } from '../api/types';
import { useI18n } from '../i18n/context';

const { Text, Paragraph } = Typography;

/** Trigger a browser download of a text file (same helper shape as Rules.tsx). */
function downloadText(filename: string, text: string) {
  const blob = new Blob([text], { type: 'text/plain' });
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = filename;
  a.click();
  URL.revokeObjectURL(url);
}

/**
 * v1.2.0: redeem-code management.
 *
 * Closes the loop the shop already assumed: the panel could DEDUCT balance
 * (buying a plan) but had no way for a user to ADD any, so balance could only
 * be typed in by an admin. Codes need no payment gateway or merchant account.
 */
export default function RedeemCodes() {
  const { t } = useI18n();
  const [items, setItems] = useState<RedeemCode[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(false);
  const [status, setStatus] = useState<string>('all');
  const [page, setPage] = useState(1);
  const [selectedRowKeys, setSelectedRowKeys] = useState<number[]>([]);
  const [createOpen, setCreateOpen] = useState(false);
  const [createForm] = Form.useForm();
  // The codes from the most recent generation, shown once so the admin can
  // copy the whole batch straight away.
  const [justCreated, setJustCreated] = useState<CreateCodesResponse | null>(null);

  const PAGE_SIZE = 20;

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const qs = new URLSearchParams({
        status,
        limit: String(PAGE_SIZE),
        offset: String((page - 1) * PAGE_SIZE),
      });
      const res = await api.get<unknown, ApiEnvelope<ListCodesResponse>>(`/admin/redeem-codes?${qs}`);
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
  }, [status, page, t]);

  useEffect(() => { load(); }, [load]);

  const handleCreate = async (values: { count: number; amount: string; expires_at?: string; remark?: string }) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<CreateCodesResponse>>('/admin/redeem-codes', {
        count: values.count,
        amount: String(values.amount),
        expires_at: values.expires_at?.trim() || undefined,
        remark: values.remark ?? '',
      });
      if (res.code !== 0) { message.error(res.message); return; }
      setJustCreated(res.data ?? null);
      setCreateOpen(false);
      createForm.resetFields();
      setPage(1);
      load();
      message.success(t('codesCreated').replace('{count}', String(res.data?.created ?? 0)));
    } catch { message.error(t('codesCreateFailed')); }
  };

  const handleVoid = async (id: number) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<null>>(`/admin/redeem-codes/${id}/void`, {});
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('codeVoided'));
      load();
    } catch { message.error(t('codeVoidFailed')); }
  };

  const handleDelete = async () => {
    if (selectedRowKeys.length === 0) return;
    try {
      const res = await api.delete<unknown, ApiEnvelope<number>>('/admin/redeem-codes', {
        data: { ids: selectedRowKeys },
      });
      if (res.code !== 0) { message.error(res.message); return; }
      const deleted = res.data ?? 0;
      const skipped = selectedRowKeys.length - deleted;
      if (skipped > 0) {
        // Used codes are never deletable — they are the record of money that
        // entered the system. Say so instead of silently deleting fewer.
        message.warning(t('codesDeletedPartial').replace('{ok}', String(deleted)).replace('{skipped}', String(skipped)));
      } else {
        message.success(t('codesDeleted').replace('{count}', String(deleted)));
      }
      setSelectedRowKeys([]);
      load();
    } catch { message.error(t('codesDeleteFailed')); }
  };

  const copyText = async (text: string, okMsg: string) => {
    try {
      await navigator.clipboard.writeText(text);
      message.success(okMsg);
    } catch {
      message.error(t('copyFailed'));
    }
  };

  const statusTag = (s: string) => {
    if (s === 'unused') return <Tag color="green">{t('codeUnused')}</Tag>;
    if (s === 'used') return <Tag>{t('codeUsed')}</Tag>;
    return <Tag color="red">{t('codeVoid')}</Tag>;
  };

  const columns = [
    {
      title: t('code'), dataIndex: 'code', key: 'code',
      render: (c: string) => (
        <Space>
          <Text code copyable={{ text: c }}>{c}</Text>
        </Space>
      ),
    },
    { title: t('faceValue'), dataIndex: 'amount', key: 'amount' },
    { title: t('status'), dataIndex: 'status', key: 'status', render: statusTag },
    {
      title: t('usedBy'), dataIndex: 'used_by', key: 'used_by',
      // used_by is nulled when the account is deleted, but the row survives —
      // it's the audit trail for money entering the system.
      render: (uid: number | null, r: RedeemCode) =>
        r.status === 'used' ? (uid ? `#${uid}` : t('deletedUser')) : '-',
    },
    { title: t('usedAt'), dataIndex: 'used_at', key: 'used_at', render: (v: string | null) => v ?? '-' },
    { title: t('codeExpiresAt'), dataIndex: 'expires_at', key: 'expires_at', render: (v: string | null) => v ?? t('neverExpires') },
    { title: t('batch'), dataIndex: 'batch_id', key: 'batch_id' },
    { title: t('remark'), dataIndex: 'remark', key: 'remark', render: (v: string) => v || '-' },
    {
      title: t('action'), key: 'action',
      render: (_: unknown, r: RedeemCode) => (
        r.status === 'unused' ? (
          <Popconfirm title={t('voidCodeConfirm')} onConfirm={() => handleVoid(r.id)} okButtonProps={{ danger: true }}>
            <Button size="small" type="text" danger icon={<StopOutlined />}>{t('void')}</Button>
          </Popconfirm>
        ) : <Text type="secondary">-</Text>
      ),
    },
  ];

  return (
    <div>
      <div style={{ display: 'flex', justifyContent: 'space-between', marginBottom: 16, flexWrap: 'wrap', gap: 8 }}>
        <Typography.Title level={4} style={{ margin: 0 }}>{t('redeemCodes')}</Typography.Title>
        <Space wrap>
          <Select
            style={{ width: 140 }}
            value={status}
            onChange={(v) => { setStatus(v); setPage(1); setSelectedRowKeys([]); }}
            options={[
              { value: 'all', label: t('allStatuses') },
              { value: 'unused', label: t('codeUnused') },
              { value: 'used', label: t('codeUsed') },
              { value: 'void', label: t('codeVoid') },
            ]}
          />
          {selectedRowKeys.length > 0 && (
            <Popconfirm
              title={t('deleteCodesConfirm').replace('{count}', String(selectedRowKeys.length))}
              description={t('deleteCodesDesc')}
              onConfirm={handleDelete}
              okButtonProps={{ danger: true }}
            >
              <Button danger icon={<DeleteOutlined />}>{t('delete')} ({selectedRowKeys.length})</Button>
            </Popconfirm>
          )}
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => { createForm.resetFields(); setCreateOpen(true); }}>
            {t('generateCodes')}
          </Button>
        </Space>
      </div>

      {justCreated && (
        <Alert
          type="success"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('justGenerated').replace('{count}', String(justCreated.created)).replace('{batch}', justCreated.batch_id)}
          description={
            <>
              <Paragraph style={{ maxHeight: 160, overflow: 'auto', marginBottom: 8, fontFamily: 'monospace' }}>
                {justCreated.codes.join('\n')}
              </Paragraph>
              <Space wrap>
                <Button size="small" icon={<CopyOutlined />} onClick={() => copyText(justCreated.codes.join('\n'), t('copied'))}>
                  {t('copyAll')}
                </Button>
                <Button size="small" icon={<DownloadOutlined />} onClick={() =>
                  downloadText(`redeem-codes-${justCreated.batch_id}.txt`, justCreated.codes.join('\n'))
                }>
                  {t('exportTxt')}
                </Button>
                <Button size="small" type="text" onClick={() => setJustCreated(null)}>{t('dismiss')}</Button>
              </Space>
            </>
          }
        />
      )}

      <Table
        rowSelection={{ selectedRowKeys, onChange: (k) => setSelectedRowKeys(k as number[]) }}
        dataSource={items}
        columns={columns}
        rowKey="id"
        loading={loading}
        scroll={{ x: 1100 }}
        pagination={{
          current: page,
          pageSize: PAGE_SIZE,
          total,
          showSizeChanger: false,
          onChange: (p) => { setPage(p); setSelectedRowKeys([]); },
        }}
      />

      <Modal
        title={t('generateCodes')}
        open={createOpen}
        onCancel={() => setCreateOpen(false)}
        onOk={() => createForm.submit()}
        okText={t('create')}
        cancelText={t('cancel')}
      >
        <Form form={createForm} onFinish={handleCreate} layout="vertical">
          <Form.Item
            name="count"
            label={t('codeCount')}
            initialValue={10}
            rules={[{ required: true, message: t('codeCountRequired') }]}
          >
            <InputNumber min={1} max={MAX_REDEEM_BATCH} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item
            name="amount"
            label={t('faceValue')}
            initialValue={10}
            extra={t('faceValueHint')}
            rules={[{ required: true, message: t('faceValueRequired') }]}
          >
            <InputNumber min={0.01} step={1} precision={2} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="expires_at" label={t('codeExpiresAt')} extra={t('expiresAtHint')}>
            <Input placeholder="2026-12-31 23:59:59" />
          </Form.Item>
          <Form.Item name="remark" label={t('remark')}>
            <Input placeholder={t('remarkPlaceholder')} />
          </Form.Item>
        </Form>
      </Modal>
    </div>
  );
}
