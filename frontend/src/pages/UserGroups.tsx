import { Table, Button, Modal, Form, Input, Select, Switch, Space, message, Popconfirm, Tag, Alert } from 'antd';
import { PlusOutlined, ReloadOutlined, EditOutlined, DeleteOutlined, InfoCircleOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, DeviceGroup } from '../api/types';
import { useI18n } from '../i18n/context';

interface UserGroup {
  id: number;
  name: string;
  remark: string;
  allow_all_groups: boolean;
  created_at: string;
}

export default function UserGroups() {
  const { t } = useI18n();
  const [groups, setGroups] = useState<UserGroup[]>([]);
  const [deviceGroups, setDeviceGroups] = useState<DeviceGroup[]>([]);
  const [loading, setLoading] = useState(false);
  const [createOpen, setCreateOpen] = useState(false);
  const [editOpen, setEditOpen] = useState(false);
  const [editing, setEditing] = useState<UserGroup | null>(null);
  const [showHelp, setShowHelp] = useState(false);
  const [createForm] = Form.useForm();
  const [editForm] = Form.useForm();

  const createAllowAll = Form.useWatch('allow_all_groups', createForm);
  const editAllowAll = Form.useWatch('allow_all_groups', editForm);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const g = await api.get<unknown, ApiEnvelope<UserGroup[]>>('/user-groups');
      setGroups(g.data || []);
      const d = await api.get<unknown, ApiEnvelope<DeviceGroup[]>>('/groups');
      setDeviceGroups((d.data || []).filter(dg => dg.group_type === 'in'));
    } catch { /* ignore */ }
    finally { setLoading(false); }
  }, []);

  useEffect(() => { load(); }, [load]);

  const inGroupOptions = deviceGroups.map(d => ({ value: d.id, label: `${d.name} (#${d.id})` }));

  const handleCreate = async (values: { name: string; remark?: string; allow_all_groups?: boolean; device_group_ids?: number[] }) => {
    const { device_group_ids, ...rest } = values;
    const res = await api.post<unknown, ApiEnvelope<UserGroup>>('/user-groups', rest);
    if (res.code !== 0) { message.error(res.message); return; }
    if (!values.allow_all_groups && device_group_ids?.length) {
      await api.put(`/user-groups/${res.data!.id}/device-groups`, { device_group_ids });
    }
    message.success(t('settingsSaved'));
    setCreateOpen(false);
    createForm.resetFields();
    load();
  };

  const handleEdit = async (g: UserGroup) => {
    setEditing(g);
    editForm.setFieldsValue({ name: g.name, remark: g.remark, allow_all_groups: g.allow_all_groups, device_group_ids: [] });
    const res = await api.get<unknown, ApiEnvelope<number[]>>(`/user-groups/${g.id}/device-groups`);
    editForm.setFieldsValue({ device_group_ids: res.data || [] });
    setEditOpen(true);
  };

  const handleUpdate = async (values: { name?: string; remark?: string; allow_all_groups?: boolean; device_group_ids?: number[] }) => {
    if (!editing) return;
    const { device_group_ids, ...rest } = values;
    const res = await api.put<unknown, ApiEnvelope<UserGroup>>(`/user-groups/${editing.id}`, rest);
    if (res.code !== 0) { message.error(res.message); return; }
    await api.put(`/user-groups/${editing.id}/device-groups`, {
      device_group_ids: values.allow_all_groups ? [] : (device_group_ids || []),
    });
    message.success(t('settingsSaved'));
    setEditOpen(false);
    load();
  };

  const handleDelete = async (id: number) => {
    try {
      const res = await api.delete<unknown, ApiEnvelope<null>>(`/user-groups/${id}`);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('groupDeleted'));
      load();
    } catch (e: unknown) {
      const err = e as { response?: { data?: { message?: string } } };
      message.error(err?.response?.data?.message || t('failedDeleteGroup'));
    }
  };

  const columns = [
    { title: 'ID', dataIndex: 'id', key: 'id', width: 60 },
    { title: t('name'), dataIndex: 'name', key: 'name' },
    { title: t('remark'), dataIndex: 'remark', key: 'remark', render: (v: string) => v || '-' },
    {
      title: t('allowAllGroups'), dataIndex: 'allow_all_groups', key: 'allow_all_groups', width: 120,
      render: (v: boolean) => v ? <Tag color="green">{t('yes')}</Tag> : <Tag>{t('no')}</Tag>,
    },
    {
      title: t('action'), key: 'action', width: 160,
      render: (_: unknown, g: UserGroup) => (
        <Space>
          <Button size="small" type="text" icon={<EditOutlined />} onClick={() => handleEdit(g)}>{t('edit')}</Button>
          <Popconfirm title={t('deleteGroupConfirm')} onConfirm={() => handleDelete(g.id)}>
            <Button danger size="small" type="text" icon={<DeleteOutlined />} />
          </Popconfirm>
        </Space>
      ),
    },
  ];

  const deviceGroupSelect = (
    <Form.Item name="device_group_ids" label={t('allowedInboundGroupsLabel')}>
      <Select mode="multiple" options={inGroupOptions} placeholder={t('selectDeviceGroups')} />
    </Form.Item>
  );

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title">{t('userGroups')}</h2>
        <Space>
          <Button icon={<InfoCircleOutlined />} onClick={() => setShowHelp(v => !v)}>{t('userGroupHelpTitle')}</Button>
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
          <Button type="primary" icon={<PlusOutlined />} onClick={() => { createForm.resetFields(); setCreateOpen(true); }}>{t('addUserGroup')}</Button>
        </Space>
      </div>

      {showHelp && (
        <Alert
          style={{ marginBottom: 16 }}
          type="info"
          showIcon
          message={t('userGroupHelpTitle')}
          description={
            <ol style={{ margin: '4px 0 0 0', paddingLeft: 20 }}>
              <li>{t('userGroupHelp1')}</li>
              <li>{t('userGroupHelp2')}</li>
              <li>{t('userGroupHelp3')}</li>
              <li>{t('userGroupHelp4')}</li>
            </ol>
          }
        />
      )}

      <Table dataSource={groups} columns={columns} rowKey="id" loading={loading} pagination={{ pageSize: 20 }} />

      <Modal title={t('addUserGroup')} open={createOpen} onCancel={() => setCreateOpen(false)} onOk={() => createForm.submit()} okText={t('create')} cancelText={t('cancel')}>
        <Form form={createForm} onFinish={handleCreate} layout="vertical" initialValues={{ allow_all_groups: false }}>
          <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input /></Form.Item>
          <Form.Item name="remark" label={t('remark')}><Input.TextArea rows={2} /></Form.Item>
          <Form.Item name="allow_all_groups" label={t('allowAllGroups')} valuePropName="checked"><Switch /></Form.Item>
          {!createAllowAll && deviceGroupSelect}
        </Form>
      </Modal>

      <Modal title={t('editUserGroup')} open={editOpen} onCancel={() => setEditOpen(false)} onOk={() => editForm.submit()} okText={t('save')} cancelText={t('cancel')}>
        <Form form={editForm} onFinish={handleUpdate} layout="vertical">
          <Form.Item name="name" label={t('name')} rules={[{ required: true }]}><Input /></Form.Item>
          <Form.Item name="remark" label={t('remark')}><Input.TextArea rows={2} /></Form.Item>
          <Form.Item name="allow_all_groups" label={t('allowAllGroups')} valuePropName="checked"><Switch /></Form.Item>
          {!editAllowAll && deviceGroupSelect}
        </Form>
      </Modal>
    </>
  );
}
