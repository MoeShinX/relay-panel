import { Table, Button, Modal, Form, Input, Select, Space, message, Popconfirm, Tag, Tooltip } from 'antd';
import { PlusOutlined, ReloadOutlined, EditOutlined, ApartmentOutlined } from '@ant-design/icons';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, TunnelProfile } from '../api/types';
import { useI18n } from '../i18n/context';

/** Form values for create/edit. transport/tls_mode/ws_path/host_header/sni. */
interface ProfileValues {
  name?: string;
  transport?: string;
  tls_mode?: string;
  ws_path?: string;
  host_header?: string;
  sni?: string;
}

export default function TunnelProfiles() {
  const { t } = useI18n();
  const [profiles, setProfiles] = useState<TunnelProfile[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editing, setEditing] = useState<TunnelProfile | null>(null);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const load = async () => {
    setLoading(true);
    try {
      // v0.4.11 PR1: use /admin/tunnel-profiles for management page (returns only
      // custom ws/tls_simple templates, not builtin).
      const res = await api.get<unknown, ApiEnvelope<TunnelProfile[]>>('/admin/tunnel-profiles');
      setProfiles(res.data || []);
    } finally { setLoading(false); }
  };

  useEffect(() => { load(); }, []);

  const handleCreate = async (values: ProfileValues) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<TunnelProfile>>('/admin/tunnel-profiles', values);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('saved'));
      setCreateOpen(false);
      createForm.resetFields();
      load();
    } catch { message.error(t('saveFailed')); }
  };

  const handleEdit = (p: TunnelProfile) => {
    setEditing(p);
    editForm.setFieldsValue({
      name: p.name,
      transport: p.transport,
      tls_mode: p.tls_mode,
      ws_path: p.ws_path,
      host_header: p.host_header,
      sni: p.sni,
    });
    setEditOpen(true);
  };

  const handleUpdate = async (values: ProfileValues) => {
    if (!editing) return;
    // Build a diff payload — only send changed fields.
    const payload: Record<string, unknown> = {};
    if (values.name !== undefined && values.name !== editing.name) payload.name = values.name;
    if (values.transport !== undefined && values.transport !== editing.transport) payload.transport = values.transport;
    if (values.tls_mode !== undefined && values.tls_mode !== editing.tls_mode) payload.tls_mode = values.tls_mode;
    if (values.ws_path !== undefined && values.ws_path !== editing.ws_path) payload.ws_path = values.ws_path;
    if (values.host_header !== undefined && values.host_header !== editing.host_header) payload.host_header = values.host_header;
    if (values.sni !== undefined && values.sni !== editing.sni) payload.sni = values.sni;
    if (Object.keys(payload).length === 0) { setEditOpen(false); return; }
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/admin/tunnel-profiles/${editing.id}`, payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('saved'));
      setEditOpen(false);
      load();
    } catch { message.error(t('saveFailed')); }
  };

  const handleDelete = async (id: number) => {
    try {
      const res = await api.delete<unknown, ApiEnvelope<null>>(`/admin/tunnel-profiles/${id}`);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('deleted'));
      load();
    } catch { message.error(t('deleteFailed')); }
  };

  // v0.4.11 PR1: direct is no longer a tunnel template concept.
  // Only ws and tls_simple are accepted.
  const transportOptions = [
    { value: 'ws', label: 'WebSocket (plaintext)' },
    { value: 'tls_simple', label: 'TLS Simple' },
  ];

  const columns = [
    { title: t('id'), dataIndex: 'id', key: 'id', width: 60 },
    {
      title: t('name'), dataIndex: 'name', key: 'name',
      render: (name: string, p: TunnelProfile) => (
        <Space>
          <span>{name}</span>
          {p.is_builtin && <Tag color="blue">{t('builtin')}</Tag>}
        </Space>
      ),
    },
    { title: t('transport'), dataIndex: 'transport', key: 'transport', render: (v: string) => <Tag>{v}</Tag> },
    { title: t('wsPath'), dataIndex: 'ws_path', key: 'ws_path', render: (v: string) => v || '-' },
    {
      title: t('action'), key: 'action', width: 120,
      render: (_: unknown, p: TunnelProfile) => (
        <Space>
          {p.is_builtin ? (
            <Tooltip title={t('builtinReadOnly')}>
              <Button size="small" type="text" icon={<EditOutlined />} disabled>{t('edit')}</Button>
            </Tooltip>
          ) : (
            <Button size="small" type="text" icon={<EditOutlined />} onClick={() => handleEdit(p)}>{t('edit')}</Button>
          )}
          {p.is_builtin ? (
            <Tooltip title={t('builtinReadOnly')}>
              <Button danger size="small" type="text" disabled>{t('delete')}</Button>
            </Tooltip>
          ) : (
            <Popconfirm title={t('deleteConfirm')} onConfirm={() => handleDelete(p.id)}>
              <Button danger size="small" type="text">{t('delete')}</Button>
            </Popconfirm>
          )}
        </Space>
      ),
    },
  ];

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title"><ApartmentOutlined /> {t('tunnelProfiles')}</h2>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>{t('addTunnelProfile')}</Button>
        </Space>
      </div>
      <Table dataSource={profiles} columns={columns} rowKey="id" loading={loading} pagination={{ pageSize: 20 }} scroll={{ x: 'max-content' }} />

      <Modal title={t('addTunnelProfile')} open={createOpen} onCancel={() => setCreateOpen(false)} onOk={() => createForm.submit()} okText={t('create')} cancelText={t('cancel')}>
        <Form form={createForm} onFinish={handleCreate} layout="vertical">
          <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input placeholder="my-wss-profile" /></Form.Item>
          <Form.Item name="transport" label={t('transport')} rules={[{ required: true }]} initialValue="ws">
            <Select options={transportOptions} />
          </Form.Item>
          {/* v0.4.7: ws_path only shown for ws transport; host_header/sni/tls_mode
              are hidden (node hasn't implemented WS Host validation or per-rule
              SNI; tls_mode isn't read by the forwarder). */}
          <Form.Item noStyle shouldUpdate={(prev, cur) => prev.transport !== cur.transport}>
            {({ getFieldValue }) => getFieldValue('transport') === 'ws' ? (
              <Form.Item name="ws_path" label={t('wsPath')}><Input placeholder="/relay" /></Form.Item>
            ) : null}
          </Form.Item>
        </Form>
      </Modal>

      <Modal title={t('editTunnelProfile')} open={editOpen} onCancel={() => setEditOpen(false)} onOk={() => editForm.submit()} okText={t('save')} cancelText={t('cancel')}>
        <Form form={editForm} onFinish={handleUpdate} layout="vertical">
          <Form.Item name="name" label={t('name')}><Input /></Form.Item>
          <Form.Item name="transport" label={t('transport')}><Select options={transportOptions} /></Form.Item>
          <Form.Item noStyle shouldUpdate={(prev, cur) => prev.transport !== cur.transport}>
            {({ getFieldValue }) => getFieldValue('transport') === 'ws' ? (
              <Form.Item name="ws_path" label={t('wsPath')}><Input /></Form.Item>
            ) : null}
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
