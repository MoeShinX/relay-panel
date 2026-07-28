import { Table, Button, Modal, Form, Input, Select, Space, message, Popconfirm, Tag } from 'antd';
import { PlusOutlined, ReloadOutlined, EditOutlined, ThunderboltOutlined, CodeOutlined, UnlockOutlined } from '@ant-design/icons';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';
import { useI18n } from '../i18n/context';

interface Tunnel {
  id: number;
  name: string;
  group_in: number;
  group_out: number | null;
  protocol: string;
  listen_port: number;
  config_json: string;
  secret_json: string;
  enabled: boolean;
  uid: number;
  created_at: string;
}

interface CreateValues {
  name: string;
  group_in: number;
  group_out?: number | null;
  protocol: string;
  listen_port: number;
}

interface UpdateValues {
  name?: string;
  group_in?: number;
  group_out?: number | null;
  protocol?: string;
  listen_port?: number;
  config_json?: string;
  secret_json?: string;
  enabled?: boolean;
}

export default function Tunnels() {
  const { t } = useI18n();
  const [tunnels, setTunnels] = useState<Tunnel[]>([]);
  const [groups, setGroups] = useState<{ id: number; name: string }[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editing, setEditing] = useState<Tunnel | null>(null);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();
  const [detailOpen, setDetailOpen] = useState(false);
  const [detailData, setDetailData] = useState<[unknown, unknown] | null>(null);

  const load = async () => {
    setLoading(true);
    try {
      const [tunRes, grpRes] = await Promise.all([
        api.get<unknown, ApiEnvelope<Tunnel[]>>('/admin/tunnels'),
        api.get<unknown, ApiEnvelope<{ id: number; name: string }[]>>('/groups'),
      ]);
      setTunnels(tunRes.data || []);
      setGroups(grpRes.data || []);
    } finally { setLoading(false); }
  };

  useEffect(() => { load(); }, []);

  const handleCreate = async (values: CreateValues) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<Tunnel>>('/admin/tunnels', values);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('saved'));
      setCreateOpen(false);
      createForm.resetFields();
      load();
    } catch { message.error(t('saveFailed')); }
  };

  const handleEdit = (tunnel: Tunnel) => {
    setEditing(tunnel);
    editForm.setFieldsValue({
      name: tunnel.name,
      group_in: tunnel.group_in,
      group_out: tunnel.group_out,
      protocol: tunnel.protocol,
      listen_port: tunnel.listen_port,
      enabled: tunnel.enabled,
    });
    setEditOpen(true);
  };

  const handleUpdate = async (values: UpdateValues) => {
    if (!editing) return;
    const payload: Record<string, unknown> = {};
    if (values.name !== undefined && values.name !== editing.name) payload.name = values.name;
    if (values.group_in !== undefined && values.group_in !== editing.group_in) payload.group_in = values.group_in;
    if (values.group_out !== undefined && values.group_out !== editing.group_out) payload.group_out = values.group_out;
    if (values.protocol !== undefined && values.protocol !== editing.protocol) payload.protocol = values.protocol;
    if (values.listen_port !== undefined && values.listen_port !== editing.listen_port) payload.listen_port = values.listen_port;
    if (values.enabled !== undefined && values.enabled !== editing.enabled) payload.enabled = values.enabled;
    if (Object.keys(payload).length === 0) { setEditOpen(false); return; }
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/admin/tunnels/${editing.id}`, payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('saved'));
      setEditOpen(false);
      load();
    } catch { message.error(t('saveFailed')); }
  };

  const handleDelete = async (id: number) => {
    try {
      const res = await api.delete<unknown, ApiEnvelope<null>>(`/admin/tunnels/${id}`);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('deleted'));
      load();
    } catch { message.error(t('deleteFailed')); }
  };

  const handleRegenerate = async (id: number) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<[unknown, unknown]>>(`/admin/tunnels/${id}/regenerate-config`);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('configRegenerated'));
      load();
    } catch { message.error(t('saveFailed')); }
  };

  const handleViewConfig = async (id: number) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<[unknown, unknown]>>(`/admin/tunnels/${id}/regenerate-config`);
      if (res.code !== 0) { message.error(res.message); return; }
      setDetailData(res.data);
      setDetailOpen(true);
    } catch { message.error(t('saveFailed')); }
  };

  const findGroupName = (gid: number): string => {
    const g = groups.find(g => g.id === gid);
    return g ? g.name : String(gid);
  };

  const columns = [
    { title: t('id'), dataIndex: 'id', key: 'id', width: 60 },
    { title: t('name'), dataIndex: 'name', key: 'name', width: 180 },
    {
      title: t('groupIn'), dataIndex: 'group_in', key: 'group_in', width: 160,
      render: (gid: number) => <Tag>{findGroupName(gid)}</Tag>,
    },
    {
      title: t('groupOut'), dataIndex: 'group_out', key: 'group_out', width: 160,
      render: (gid: number | null) => gid ? <Tag color="green">{findGroupName(gid)}</Tag> : <span style={{ color: '#999' }}>—</span>,
    },
    { title: t('protocol'), dataIndex: 'protocol', key: 'protocol', width: 130, render: (v: string) => <Tag>{v}</Tag> },
    { title: t('listenPort'), dataIndex: 'listen_port', key: 'listen_port', width: 100 },
    {
      title: t('enabled'), dataIndex: 'enabled', key: 'enabled', width: 90,
      render: (v: boolean) => <Tag color={v ? 'green' : 'default'}>{v ? t('yes') : t('no')}</Tag>,
    },
    {
      title: t('action'), key: 'action', width: 200,
      render: (_: unknown, rec: Tunnel) => (
        <Space>
          <Button size="small" icon={<CodeOutlined />} onClick={() => handleViewConfig(rec.id)}>{t('viewConfig')}</Button>
          <Button size="small" icon={<ThunderboltOutlined />} onClick={() => handleRegenerate(rec.id)}>{t('regenerate')}</Button>
          <Button size="small" type="text" icon={<EditOutlined />} onClick={() => handleEdit(rec)}>{t('edit')}</Button>
          <Popconfirm title={t('deleteConfirm')} onConfirm={() => handleDelete(rec.id)}>
            <Button danger size="small" type="text">{t('delete')}</Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title"><UnlockOutlined /> {t('tunnels')}</h2>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>{t('addTunnel')}</Button>
        </Space>
      </div>
      <Table dataSource={tunnels} columns={columns} rowKey="id" loading={loading} pagination={{ pageSize: 20 }} />

      {/* Create */}
      <Modal title={t('addTunnel')} open={createOpen} onCancel={() => setCreateOpen(false)} onOk={() => createForm.submit()} okText={t('create')} cancelText={t('cancel')}>
        <Form form={createForm} onFinish={handleCreate} layout="vertical">
          <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input placeholder="my-tunnel" /></Form.Item>
          <Form.Item name="group_in" label={t('groupIn')} rules={[{ required: true }]}>
            <Select options={groups.map(g => ({ value: g.id, label: g.name }))} />
          </Form.Item>
          <Form.Item name="group_out" label={t('groupOut')} tooltip={t('groupOutTooltip')}>
            <Select allowClear options={groups.map(g => ({ value: g.id, label: g.name }))} />
          </Form.Item>
          <Form.Item name="protocol" label={t('protocol')} initialValue="vless_reality">
            <Select options={[{ value: 'vless_reality', label: 'vless reality' }]} />
          </Form.Item>
          <Form.Item name="listen_port" label={t('listenPort')} rules={[{ required: true }]} initialValue={443}>
            <Input type="number" min={1} max={65535} />
          </Form.Item>
        </Form>
      </Modal>

      {/* Edit */}
      <Modal title={t('editTunnel')} open={editOpen} onCancel={() => setEditOpen(false)} onOk={() => editForm.submit()} okText={t('save')} cancelText={t('cancel')}>
        <Form form={editForm} onFinish={handleUpdate} layout="vertical">
          <Form.Item name="name" label={t('name')}><Input /></Form.Item>
          <Form.Item name="group_in" label={t('groupIn')}>
            <Select options={groups.map(g => ({ value: g.id, label: g.name }))} />
          </Form.Item>
          <Form.Item name="group_out" label={t('groupOut')} tooltip={t('groupOutTooltip')}>
            <Select allowClear options={groups.map(g => ({ value: g.id, label: g.name }))} />
          </Form.Item>
          <Form.Item name="protocol" label={t('protocol')}>
            <Select options={[{ value: 'vless_reality', label: 'vless reality' }]} />
          </Form.Item>
          <Form.Item name="listen_port" label={t('listenPort')} rules={[{ required: true }]}>
            <Input type="number" min={1} max={65535} />
          </Form.Item>
          <Form.Item name="enabled" label="启用" valuePropName="checked">
            <Select options={[{ value: true, label: '是' }, { value: false, label: '否' }]} />
          </Form.Item>
        </Form>
      </Modal>

      {/* Config viewer */}
      <Modal title={t('configViewer')} open={detailOpen} onCancel={() => setDetailOpen(false)} footer={null} width={800}>
        {detailData && (
          <div>
            <h4>{t('singBoxConfig')}</h4>
            <pre style={{ background: '#f5f5f5', padding: 12, borderRadius: 4, maxHeight: 400, overflow: 'auto' }}>
              {JSON.stringify(detailData[0], null, 2)}
            </pre>
            <h4>{t('secret')}</h4>
            <pre style={{ background: '#f5f5f5', padding: 12, borderRadius: 4, maxHeight: 400, overflow: 'auto' }}>
              {JSON.stringify(detailData[1], null, 2)}
            </pre>
          </div>
        )}
      </Modal>
    </>
  );
}
