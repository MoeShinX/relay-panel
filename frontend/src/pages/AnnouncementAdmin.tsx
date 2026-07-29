import {
  Card, Table, Button, Modal, Form, Input, Select, Switch, Space, Tag, message, Popconfirm, Alert, Typography,
} from 'antd';
import { PlusOutlined, ReloadOutlined, EditOutlined, DeleteOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Announcement, AnnouncementList } from '../api/types';
import { useI18n } from '../i18n/context';
import { renderMarkdown } from '../utils/markdown';
import { invalidateActiveAnnouncement } from '../hooks/useActiveAnnouncement';
import { kindLabel } from '../utils/announcementKind';

const { Text } = Typography;

const PAGE_SIZE = 20;
const MAX_TITLE = 120;
const MAX_CONTENT = 4000;

const KINDS = ['info', 'success', 'warning', 'error'] as const;
type Kind = (typeof KINDS)[number];

const TAG_COLOR: Record<string, string> = {
  info: 'blue',
  success: 'green',
  warning: 'orange',
  error: 'red',
};

interface FormValues {
  title: string;
  content: string;
  kind: Kind;
  pinned: boolean;
  expires_at?: string;
}

/**
 * v1.3.0: announcement management.
 *
 * Replaces the single announcement field on the site-settings page. That field
 * held one notice and overwrote it on every edit; this keeps every notice, so
 * users get an archive and the operator can look back at what was said.
 */
export default function AnnouncementAdmin() {
  const { t } = useI18n();
  const [items, setItems] = useState<Announcement[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(false);
  const [editing, setEditing] = useState<Announcement | null>(null);
  const [open, setOpen] = useState(false);
  const [saving, setSaving] = useState(false);
  const [form] = Form.useForm<FormValues>();

  const content = Form.useWatch('content', form);
  const kind = Form.useWatch('kind', form);
  const previewKind: Kind = (KINDS as readonly string[]).includes(kind) ? (kind as Kind) : 'info';

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const qs = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String((page - 1) * PAGE_SIZE),
      });
      const res = await api.get<unknown, ApiEnvelope<AnnouncementList>>(
        `/admin/announcements?${qs}`,
      );
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('loadFailed'));
        return;
      }
      setItems(res.data.items);
      setTotal(res.data.total);
    } catch {
      message.error(t('loadFailed'));
    } finally {
      setLoading(false);
    }
  }, [page, t]);

  useEffect(() => { load(); }, [load]);

  const openCreate = () => {
    setEditing(null);
    form.resetFields();
    form.setFieldsValue({ kind: 'info', pinned: false, title: '', content: '' });
    setOpen(true);
  };

  const openEdit = (a: Announcement) => {
    setEditing(a);
    form.setFieldsValue({
      title: a.title,
      content: a.content,
      kind: (KINDS as readonly string[]).includes(a.kind) ? (a.kind as Kind) : 'info',
      pinned: a.pinned,
      expires_at: a.expires_at ?? '',
    });
    setOpen(true);
  };

  const submit = async () => {
    const v = await form.validateFields();
    setSaving(true);
    try {
      const body = { ...v, expires_at: v.expires_at?.trim() || null };
      const res = editing
        ? await api.put<unknown, ApiEnvelope<null>>(`/admin/announcements/${editing.id}`, body)
        : await api.post<unknown, ApiEnvelope<number>>('/admin/announcements', body);
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      setOpen(false);
      // The banner is cached module-wide; drop it so a new notice shows without
      // a full reload.
      invalidateActiveAnnouncement();
      message.success(t('settingsSaved'));
      load();
    } catch {
      message.error(t('settingsSaveFailed'));
    } finally {
      setSaving(false);
    }
  };

  const remove = async (id: number) => {
    try {
      const res = await api.delete<unknown, ApiEnvelope<null>>(`/admin/announcements/${id}`);
      if (res.code !== 0) { message.error(res.message); return; }
      invalidateActiveAnnouncement();
      message.success(t('deleted'));
      load();
    } catch {
      message.error(t('deleteFailed'));
    }
  };

  const columns = [
    {
      title: t('announcementTitle'),
      key: 'title',
      render: (_: unknown, a: Announcement) => (
        <Space direction="vertical" size={0}>
          <Space size={4} wrap>
            <Tag color={TAG_COLOR[a.kind] ?? 'blue'}>{kindLabel(t, a.kind)}</Tag>
            {a.pinned && <Tag color="gold">{t('pinned')}</Tag>}
            <Text strong>{a.title || '-'}</Text>
          </Space>
          <Text type="secondary" style={{ fontSize: 12 }} ellipsis>
            {a.content.slice(0, 60)}
          </Text>
        </Space>
      ),
    },
    {
      title: t('publishedAt'),
      dataIndex: 'published_at',
      key: 'published_at',
      width: 170,
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('expiresAt'),
      dataIndex: 'expires_at',
      key: 'expires_at',
      width: 170,
      render: (v: string | null) =>
        v ? <span className="rp-mono">{v}</span> : <Text type="secondary">{t('neverExpires')}</Text>,
    },
    {
      title: t('auditActor'),
      dataIndex: 'author_name',
      key: 'author_name',
      width: 120,
      render: (v: string) => v || '-',
    },
    {
      title: t('actions'),
      key: 'actions',
      width: 140,
      render: (_: unknown, a: Announcement) => (
        <Space>
          <Button size="small" icon={<EditOutlined />} onClick={() => openEdit(a)} />
          <Popconfirm title={t('confirmDelete')} onConfirm={() => remove(a.id)}>
            <Button size="small" danger icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <Card
      title={t('announcementAdmin')}
      extra={
        <Space>
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>
            {t('newAnnouncement')}
          </Button>
        </Space>
      }
    >
      <Table
        rowKey="id"
        loading={loading}
        columns={columns}
        dataSource={items}
        scroll={{ x: 'max-content' }}
        pagination={{ current: page, pageSize: PAGE_SIZE, total, showSizeChanger: false, onChange: setPage }}
      />

      <Modal
        open={open}
        title={editing ? t('editAnnouncement') : t('newAnnouncement')}
        onCancel={() => setOpen(false)}
        onOk={submit}
        confirmLoading={saving}
        okText={t('save')}
        cancelText={t('cancel')}
        width={680}
        destroyOnHidden
      >
        <Form form={form} layout="vertical">
          <Form.Item name="title" label={t('announcementTitle')}>
            <Input showCount maxLength={MAX_TITLE} />
          </Form.Item>
          <Form.Item
            name="kind"
            label={t('siteAnnouncementType')}
          >
            <Select
              style={{ maxWidth: 220 }}
              options={[
                { value: 'info', label: t('announcementTypeInfo') },
                { value: 'success', label: t('announcementTypeSuccess') },
                { value: 'warning', label: t('announcementTypeWarning') },
                { value: 'error', label: t('announcementTypeError') },
              ]}
            />
          </Form.Item>
          <Form.Item
            name="content"
            label={t('siteAnnouncement')}
            rules={[{ required: true, message: t('announcementContentRequired') }]}
            style={{ marginBottom: 28 }}
          >
            <Input.TextArea rows={6} showCount maxLength={MAX_CONTENT} />
          </Form.Item>
          <Text type="secondary" style={{ fontSize: 12, display: 'block', marginBottom: 16 }}>
            {t('siteAnnouncementSyntax')}
          </Text>
          {content?.trim() ? (
            <Form.Item label={t('siteAnnouncementPreview')}>
              <Alert
                type={previewKind}
                showIcon
                description={<div style={{ whiteSpace: 'pre-wrap' }}>{renderMarkdown(content)}</div>}
              />
            </Form.Item>
          ) : null}
          <Form.Item
            name="expires_at"
            label={t('expiresAt')}
            extra={t('expiresAtHint')}
          >
            <Input placeholder="2026-08-01 12:00:00" />
          </Form.Item>
          <Form.Item name="pinned" label={t('pinned')} valuePropName="checked" extra={t('pinnedHint')}>
            <Switch />
          </Form.Item>
        </Form>
      </Modal>
    </Card>
  );
}
