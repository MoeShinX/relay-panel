import { Table, Button, Modal, Form, Input, InputNumber, Select, Space, message, Popconfirm, Typography, Tag, Tooltip, Alert, Switch } from 'antd';
import { PlusOutlined, ReloadOutlined, CopyOutlined, EditOutlined, CloudServerOutlined, CodeOutlined, ApiOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState, type ReactNode } from 'react';
import api from '../api/client';
import type { ApiEnvelope, DeviceGroup, User, NodeStatus } from '../api/types';
import { useI18n } from '../i18n/context';
import { copyText } from '../utils/clipboard';
import { useAuth } from '../auth/useAuth';

const { Text } = Typography;

const INSTALL_SCRIPT_URL = 'https://raw.githubusercontent.com/MoeShinX/relay-panel/main/scripts/relay-node-install.sh';

function buildInstallCommand(token: string, panelUrl: string): string {
  return `bash <(curl -fsSL ${INSTALL_SCRIPT_URL}) -t ${token} -u ${panelUrl}`;
}

function isLocalhost(): boolean {
  const h = window.location.hostname;
  return h === 'localhost' || h === '127.0.0.1' || h === '::1';
}

export default function Groups() {
  const { t } = useI18n();
  const { isAdmin } = useAuth();
  const [groups, setGroups] = useState<DeviceGroup[]>([]);
  const [users, setUsers] = useState<User[]>([]);
  const [nodes, setNodes] = useState<NodeStatus[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [cmdModalOpen, setCmdModalOpen] = useState(false);
  const [cmdModalContent, setCmdModalContent] = useState<{ title: ReactNode; body: ReactNode }>({ title: null, body: null });
  const [editing, setEditing] = useState<DeviceGroup | null>(null);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const g = await api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups');
      setGroups(g.data || []);
      if (isAdmin) {
        try {
          const u = await api.get<unknown, ApiEnvelope<User[]>>('/admin/users');
          setUsers(u.data || []);
        } catch { setUsers([]); }
        // v1.0.4: fetch node status for expandable node lists.
        try {
          const n = await api.get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes');
          setNodes(n.data || []);
        } catch { setNodes([]); }
      } else {
        setUsers([]);
        try {
          const n = await api.get<unknown, ApiEnvelope<NodeStatus[]>>('/nodes/shared');
          setNodes(n.data || []);
        } catch { setNodes([]); }
      }
    } finally { setLoading(false); }
  }, [isAdmin]);

  useEffect(() => { load(); }, [load]);

  // ── Node helpers ──
  const nodesByGroup = useCallback((groupId: number): NodeStatus[] => {
    return nodes.filter(n => n.group_id === groupId);
  }, [nodes]);

  const nodeCount = useCallback((groupId: number) => nodesByGroup(groupId).length, [nodesByGroup]);
  const onlineCount = useCallback((groupId: number) => nodesByGroup(groupId).filter(n => n.online).length, [nodesByGroup]);

  const handleCreate = async (values: { name: string; group_type: string; connect_host: string; port_range: string; rate?: number; hidden?: boolean; owner_uid?: number | null }) => {
    try {
      // v1.0.8: rate defaults to 1.0 on the server when omitted; send it
      // explicitly so the value the admin picked is what gets persisted.
      const payload = { ...values, rate: values.rate ?? 1.0, hidden: values.hidden ?? false, owner_uid: values.owner_uid || undefined };
      const res = await api.post<unknown, ApiEnvelope<DeviceGroup>>('/groups', payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('groupCreated'));
      setCreateOpen(false);
      createForm.resetFields();
      load();
    } catch { message.error(t('failedCreateGroup')); }
  };

  const handleEdit = (g: DeviceGroup) => {
    setEditing(g);
    editForm.setFieldsValue({ name: g.name, group_type: g.group_type, connect_host: g.connect_host, port_range: g.port_range, rate: g.rate, hidden: !!g.hidden });
    setEditOpen(true);
  };

  const handleUpdate = async (values: { name?: string; group_type?: string; connect_host?: string; port_range?: string; rate?: number; hidden?: boolean }) => {
    if (!editing) return;
    const payload: Record<string, unknown> = {};
    if (values.name !== undefined && values.name !== editing.name) payload.name = values.name;
    if (values.group_type !== undefined && values.group_type !== editing.group_type) payload.group_type = values.group_type;
    if (values.connect_host !== undefined && values.connect_host !== editing.connect_host) payload.connect_host = values.connect_host;
    if (values.port_range !== undefined && values.port_range !== editing.port_range) payload.port_range = values.port_range;
    // v1.0.8: only send rate when it actually changed (avoid no-op 400s and
    // keep the diff-based payload pattern used for the other fields).
    if (values.rate !== undefined && values.rate !== editing.rate) payload.rate = values.rate;
    // v1.0.7: only send hidden when it actually changed.
    if (values.hidden !== undefined && values.hidden !== !!editing.hidden) payload.hidden = values.hidden;
    if (Object.keys(payload).length === 0) { setEditOpen(false); return; }
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/groups/${editing.id}`, payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('groupUpdated'));
      setEditOpen(false);
      load();
    } catch { message.error(t('failedUpdateGroup')); }
  };

  const handleDelete = async (id: number) => {
    try {
      await api.delete(`/groups/${id}`);
      message.success(t('groupDeleted'));
      load();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { code?: number; message?: string } } };
      if (err?.response?.data?.code === 409) {
        message.error(err.response.data.message || t('groupInUse'));
      } else {
        message.error(t('failedDeleteGroup'));
      }
    }
  };

  const doCopy = async (text: string, successMsg: string) => {
    if (!text || text.length < 20) { message.error(t('copyFailed')); return; }
    const ok = await copyText(text);
    if (ok) {
      message.success(successMsg);
    } else {
      message.error(t('copyFailed'));
    }
  };

  const panelUrlRef = async (): Promise<string> => {
    try {
      const resp = await api.get<unknown, { public_panel_url?: string }>("/system/version");
      if (resp.public_panel_url) return resp.public_panel_url;
    } catch { /* ignore */ }
    return window.location.origin;
  };

  const showInstallCommand = async (g: DeviceGroup) => {
    const panelUrl = await panelUrlRef();
    const cmd = buildInstallCommand(g.token, panelUrl);
    setCmdModalContent({
      title: <span>{t('installCommandTitle')}</span>,
      body: (
        <>
          {(isLocalhost() || panelUrl.includes("127.0.0.1") || panelUrl.includes("localhost") || panelUrl.includes("0.0.0.0")) && (
            <Alert type="warning" showIcon style={{ marginBottom: 12 }} title={t('localhostWarning')} />
          )}
          <Input.TextArea value={cmd} readOnly autoSize={{ minRows: 3, maxRows: 5 }} style={{ fontFamily: 'var(--rp-font-mono)', fontSize: 12 }} />
          <div style={{ textAlign: 'right', marginTop: 8 }}>
            <Button type="primary" icon={<CopyOutlined />} onClick={() => doCopy(cmd, t('installCommandCopied'))}>
              {t('copyInstallCommand')}
            </Button>
          </div>
        </>
      ),
    });
    setCmdModalOpen(true);
  };

  const typeColor = (gt: string) => {
    switch (gt) {
      case 'in': return 'green';
      case 'out': return 'cyan';
      case 'monitor': return 'default';
      default: return 'default';
    }
  };

  // v1.0.4: create form only shows in/monitor (no out/egress).
  // v1.0.9: the edit form uses the same set — outbound/egress groups are no
  // longer offered anywhere in the UI.
  const groupTypeOptions = [
    { value: 'in', label: t('inboundListener') },
    { value: 'monitor', label: t('typeMonitor') },
  ];

  const columns = [
    { title: t('id'), dataIndex: 'id', key: 'id', width: 60 },
    { title: t('name'), dataIndex: 'name', key: 'name' },
    {
      title: t('type'), dataIndex: 'group_type', key: 'group_type',
      render: (gt: string) => <Tag color={typeColor(gt)}>{gt.toUpperCase()}</Tag>,
    },
    {
      title: t('nodes'), key: 'nodes', width: 100,
      render: (_: unknown, g: DeviceGroup) => {
        const total = nodeCount(g.id);
        const online = onlineCount(g.id);
        return <span>{total > 0 ? `${online}/${total}` : '-'}</span>;
      },
    },
    {
      title: t('nodeToken'), dataIndex: 'token', key: 'token',
      render: (tk: string, g: DeviceGroup) => (
        <Space>
          <Text code style={{ maxWidth: 180 }} ellipsis>{tk}</Text>
          <Tooltip title={t('copyInstallCommand')}>
            <Button size="small" type="text" icon={<CodeOutlined />} aria-label={t('copyInstallCommand')} onClick={() => showInstallCommand(g)} />
          </Tooltip>
        </Space>
      ),
    },
    { title: t('connectHost'), dataIndex: 'connect_host', key: 'connect_host', render: (v: string) => <span className="rp-mono">{v}</span> },
    { title: t('portRange'), dataIndex: 'port_range', key: 'port_range', render: (v: string) => <span className="rp-mono">{v}</span> },
    {
      // v1.0.8: billing rate. Only show a tag when it differs from 1.0 — a 1x
      // column on every row is noise. The tag color reflects the multiplier
      // direction (gold = premium line, no tag = bill-as-used).
      title: t('rate'), dataIndex: 'rate', key: 'rate', width: 80,
      render: (rate: number) => {
        const r = typeof rate === 'number' ? rate : 1.0;
        if (Math.abs(r - 1.0) < 1e-9) return <span style={{ color: 'var(--rp-text-tertiary)' }}>1x</span>;
        // Trim trailing zeros: 2.0 → "2x", 1.5 → "1.5x".
        const label = Number.isInteger(r) ? `${r}x` : `${r}x`;
        return <Tag color="gold">{label}</Tag>;
      },
    },
    {
      // v1.0.7: hidden flag — only tag when hidden, to keep the column quiet.
      title: t('groupHidden'), dataIndex: 'hidden', key: 'hidden', width: 80,
      render: (hidden: boolean) =>
        hidden ? <Tag>{t('yes')}</Tag> : <span style={{ color: 'var(--rp-text-tertiary)' }}>-</span>,
    },
    {
      title: t('action'), key: 'action', width: 120,
      render: (_: unknown, g: DeviceGroup) => (
        <Space>
          <Button size="small" type="text" icon={<EditOutlined />} onClick={() => handleEdit(g)}>{t('edit')}</Button>
          <Popconfirm title={t('deleteGroupConfirm')} onConfirm={() => handleDelete(g.id)}>
            <Button danger size="small" type="text">{t('delete')}</Button>
          </Popconfirm>
        </Space>
      ),
    },
  ];

  const expandedRowRender = (g: DeviceGroup) => {
    const groupNodes = nodesByGroup(g.id);
    if (groupNodes.length === 0) {
      return (
        <div style={{ padding: '8px 0', color: 'var(--rp-text-tertiary)', fontSize: 13 }}>
          {t('noNodesInGroup')}
          <Button size="small" type="link" icon={<ApiOutlined />} style={{ marginLeft: 12 }} onClick={() => showInstallCommand(g)}>
            {t('addNode')}
          </Button>
        </div>
      );
    }
    return (
      <div style={{ padding: 4 }}>
        <div style={{ marginBottom: 8, display: 'flex', justifyContent: 'space-between', alignItems: 'center' }}>
          <Text type="secondary" style={{ fontSize: 12 }}>{t('nodesInGroup')} ({groupNodes.length})</Text>
          <Button size="small" icon={<ApiOutlined />} onClick={() => showInstallCommand(g)}>{t('addNode')}</Button>
        </div>
        <Table
          dataSource={groupNodes}
          rowKey={(n: NodeStatus) => n.node_id ?? `${n.public_ipv4 ?? n.public_ip}-${n.last_seen}`}
          pagination={false}
          size="small"
          columns={[
            { title: 'ID', dataIndex: 'node_id', key: 'node_id', width: 120, render: (v: string | undefined) => v ? <Text code style={{ fontSize: 11 }}>{v.slice(0, 8)}...{v.slice(-4)}</Text> : '-' },
            { title: t('status'), dataIndex: 'online', key: 'online', width: 80, render: (v: boolean) => <Tag color={v ? 'green' : 'default'}>{v ? t('online') : t('offline')}</Tag> },
            { title: t('nodeVersion'), dataIndex: 'node_version', key: 'version', width: 90, render: (v: string | undefined) => v ? <span className="rp-mono" style={{ fontSize: 12 }}>{v}</span> : '-' },
            { title: t('lastSeen'), dataIndex: 'last_seen', key: 'last_seen', width: 120, render: (v: string | undefined) => v ? <span style={{ fontSize: 12 }}>{v}</span> : '-' },
          ]}
        />
      </div>
    );
  };

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title"><CloudServerOutlined /> {t('deviceGroups')}</h2>
        <Space>
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => setCreateOpen(true)}>{t('addGroup')}</Button>
        </Space>
      </div>
      <Table
        dataSource={groups}
        columns={columns}
        rowKey="id"
        loading={loading}
        pagination={{ pageSize: 20 }}
        expandable={{
          expandedRowRender,
          rowExpandable: () => true,
        }}
      />

      <Modal title={t('addGroup')} open={createOpen} onCancel={() => setCreateOpen(false)} onOk={() => createForm.submit()} okText={t('create')} cancelText={t('cancel')}>
        <Form form={createForm} onFinish={handleCreate} layout="vertical">
          <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input placeholder="tokyo-node-1" /></Form.Item>
          {isAdmin && (
            <Form.Item name="owner_uid" label={t('owner')} extra={t('ownerHint')}>
              <Select allowClear placeholder={t('ownerSelf')} options={users.map(u => ({ value: u.id, label: u.username }))} />
            </Form.Item>
          )}
          {/* v1.0.4: new groups cannot be type 'out' (egress). */}
          <Form.Item name="group_type" label={t('type')} rules={[{ required: true }]} initialValue="in">
            <Select options={groupTypeOptions} />
          </Form.Item>
          <Form.Item name="connect_host" label={t('connectHost')} rules={[{ required: true }]}><Input placeholder="1.2.3.4 or node.example.com" /></Form.Item>
          <Form.Item name="port_range" label={t('portRange')} rules={[{ required: true }]} initialValue="10000-65535"><Input placeholder="10000-65535" /></Form.Item>
          {/* v1.0.8: billing rate. Users are charged real bytes × rate; the
              rule/user byte counters keep real bytes. 1.0 = bill as used. */}
          <Form.Item name="rate" label={t('rate')} initialValue={1.0} extra={t('rateHint')} rules={[{ required: true }]}>
            <InputNumber min={0.1} max={100} step={0.1} style={{ width: '100%' }} />
          </Form.Item>
          {/* v1.0.7: hide this group from regular users' node-status / available
              lines. Admins always see it. */}
          <Form.Item name="hidden" label={t('groupHidden')} valuePropName="checked" initialValue={false} extra={t('groupHiddenHint')}>
            <Switch />
          </Form.Item>
        </Form>
      </Modal>

      <Modal title={t('editGroup')} open={editOpen} onCancel={() => setEditOpen(false)} onOk={() => editForm.submit()} okText={t('save')} cancelText={t('cancel')}>
        <Form form={editForm} onFinish={handleUpdate} layout="vertical">
          <Form.Item name="name" label={t('name')}><Input /></Form.Item>
          <Form.Item name="group_type" label={t('type')}><Select options={groupTypeOptions} /></Form.Item>
          <Form.Item name="connect_host" label={t('connectHost')}><Input /></Form.Item>
          <Form.Item name="port_range" label={t('portRange')}><Input /></Form.Item>
          <Form.Item name="rate" label={t('rate')} extra={t('rateHint')}>
            <InputNumber min={0.1} max={100} step={0.1} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item name="hidden" label={t('groupHidden')} valuePropName="checked" extra={t('groupHiddenHint')}>
            <Switch />
          </Form.Item>
        </Form>
      </Modal>

      <Modal title={cmdModalContent.title} open={cmdModalOpen} onCancel={() => setCmdModalOpen(false)} footer={null} width={580}>
        {cmdModalContent.body}
      </Modal>
    </>
  );
}
