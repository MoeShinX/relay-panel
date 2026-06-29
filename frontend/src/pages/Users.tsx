import { Table, Button, Tag, Popconfirm, message, Progress, Tooltip, Modal, Form, Input, InputNumber, Switch, Space, Select } from 'antd';
import { EditOutlined, ReloadOutlined, UndoOutlined, UserOutlined, PlusOutlined, KeyOutlined, ApiOutlined } from '@ant-design/icons';
import { useEffect, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, User } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';
import { useAuth } from '../auth/useAuth';

// traffic_limit is stored in BYTES in the database. The edit form works in GB
// for usability (a raw byte count is meaningless to a human). Convert on the
// boundary only — the backend and DB stay byte-based.
const BYTES_PER_GB = 1024 * 1024 * 1024;
const bytesToGb = (bytes: number): number =>
  bytes > 0 ? Math.round((bytes / BYTES_PER_GB) * 100) / 100 : 0;
const gbToBytes = (gb: number): number => Math.round(gb * BYTES_PER_GB);

interface UserFormValues {
  // Stored as a string (not number) so the wire format matches the backend's
  // TEXT-typed `users.balance` column and the strict `parse_balance` rules
  // in crates/shared/src/money.rs. InputNumber with `stringMode` keeps the
  // value as a string end-to-end.
  balance: string | null;
  max_rules: number;
  // Edited in GB; converted to bytes before sending to the backend.
  traffic_limit_gb: number;
  banned: boolean;
  group_id?: number | null;
}

interface CreateUserFormValues {
  username: string;
  password: string;
}

// v0.4.10 PR4: admin password-reset form.
interface ResetFormValues {
  new_password: string;
  confirm_password: string;
  must_change_password: boolean;
}

export default function Users() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const [users, setUsers] = useState<User[]>([]);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [editing, setEditing] = useState<User | null>(null);
  const [creating, setCreating] = useState(false);
  // v0.4.10 PR4: admin password reset state. resetting = the target user row.
  const [resetting, setResetting] = useState<User | null>(null);
  const [userGroups, setUserGroups] = useState<{ id: number; name: string }[]>([]);
  const [form] = Form.useForm<UserFormValues>();
  const [createForm] = Form.useForm<CreateUserFormValues>();
  const [resetForm] = Form.useForm<ResetFormValues>();

  // Only admins can create users / delete regular users. v0.4.10: read from
  // AuthContext (server-verified role) instead of localStorage. The backend
  // enforces this independently — this only governs UI affordances. (Users.tsx
  // is itself behind RequireAdmin, so isAdmin is effectively always true here,
  // but we keep the guard for clarity + future reuse.)
  const { isAdmin } = useAuth();

  const load = async () => {
    setLoading(true);
    try {
      const usersRes = await api.get<unknown, ApiEnvelope<User[]>>('/admin/users');
      setUsers(usersRes.data || []);
      // v1.0.4: load user permission groups for the group_id selector.
      try {
        const gRes = await api.get<unknown, ApiEnvelope<{ id: number; name: string }[]>>('/user-groups');
        setUserGroups(gRes.data || []);
      } catch { setUserGroups([]); }
    } finally { setLoading(false); }
  };

  useEffect(() => { load(); }, []);

  const handleDelete = async (id: number) => {
    const res = await api.delete<unknown, ApiEnvelope<null>>(`/admin/users/${id}`);
    if (res.code !== 0) { message.error(res.message); return; }
    message.success(t('userDeleted'));
    load();
  };

  const openEdit = (u: User) => {
    setEditing(u);
    form.setFieldsValue({
      // InputNumber with stringMode wants a string. Existing rows already have
      // a canonical TEXT-form value (e.g. "12.30"); pass it through unchanged.
      balance: u.balance,
      max_rules: u.max_rules,
      // DB stores bytes; show GB in the form.
      traffic_limit_gb: bytesToGb(u.traffic_limit),
      banned: u.banned,
      group_id: u.group_id,
    });
  };

  const handleSave = async () => {
    if (!editing) return;
    const values = await form.validateFields();
    // Trim the balance string and convert empty input to undefined so the
    // backend leaves the column unchanged. The strict validator below ensures
    // we only ever forward a value the backend will accept.
    const balance = typeof values.balance === 'string' ? values.balance.trim() : '';
    const payload: Record<string, unknown> = {
      max_rules: values.max_rules,
      banned: values.banned,
      // Convert GB back to the byte count the backend/DB expect.
      traffic_limit: gbToBytes(values.traffic_limit_gb),
    };
    if (balance !== '') {
      payload.balance = balance;
    }
    // v1.0.4: send group_id if changed.
    if (values.group_id !== editing.group_id) {
      payload.group_id = values.group_id;
    }
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(`/admin/users/${editing.id}`, payload);
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('userUpdated'));
      setEditing(null);
      load();
    } finally { setSaving(false); }
  };

  const openCreate = () => {
    createForm.resetFields();
    setCreating(true);
  };

  const handleCreate = async () => {
    const values = await createForm.validateFields();
    setSaving(true);
    try {
      const res = await api.post<unknown, ApiEnvelope<null>>('/admin/users', {
        username: values.username,
        password: values.password,
      });
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('userCreated'));
      setCreating(false);
      load();
    } finally { setSaving(false); }
  };

  const handleResetTraffic = async (id: number) => {
    const res = await api.post<unknown, ApiEnvelope<null>>(`/admin/users/${id}/reset-traffic`);
    if (res.code !== 0) { message.error(res.message); return; }
    message.success(t('trafficReset'));
    load();
  };

  // v0.4.10 PR4: open the admin password-reset modal for a user.
  const openReset = (u: User) => {
    setResetting(u);
    resetForm.resetFields();
    // Default: force the user to change this temporary password on next login.
    resetForm.setFieldsValue({ must_change_password: true });
  };

  const handleReset = async () => {
    if (!resetting) return;
    const values = await resetForm.validateFields();
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>(
        `/admin/users/${resetting.id}/password`,
        {
          new_password: values.new_password,
          must_change_password: values.must_change_password,
        }
      );
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('passwordResetSuccess'));
      setResetting(null);
    } finally { setSaving(false); }
  };

  const columns = [
    { title: t('id'), dataIndex: 'id', key: 'id', width: 60 },
    { title: t('username'), dataIndex: 'username', key: 'username' },
    {
      title: t('role'), dataIndex: 'admin', key: 'admin',
      render: (a: boolean) => a ? <Tag color="gold">{t('admin')}</Tag> : <Tag>{t('user')}</Tag>,
    },
    {
      title: t('status'), dataIndex: 'banned', key: 'banned',
      render: (b: boolean) => b ? <Tag color="red">{t('banned')}</Tag> : <Tag color="green">{t('active')}</Tag>,
    },
    { title: t('balance'), dataIndex: 'balance', key: 'balance' },
    {
      title: t('userGroups'), dataIndex: 'group_id', key: 'group_id', width: 100,
      render: (gid: number | null) => {
        const g = userGroups.find(x => x.id === gid);
        return g ? <Tag>{g.name}</Tag> : '-';
      },
    },
    { title: t('maxRules'), dataIndex: 'max_rules', key: 'max_rules' },
    {
      title: t('trafficUsed'), key: 'traffic', width: 200,
      render: (_: unknown, u: User) => {
        const used = u.traffic_used;
        const limit = u.traffic_limit;
        const unlimited = limit === 0;
        const pct = unlimited ? 0 : Math.min(100, Math.round((used / limit) * 100));
        const overQuota = !unlimited && used >= limit;
        const remaining = unlimited ? null : Math.max(0, limit - used);
        return (
          <Tooltip
            title={
              `${t('trafficUsed')}: ${formatBytes(used)}\n` +
              `${t('trafficLimit')}: ${unlimited ? t('unlimited') : formatBytes(limit)}\n` +
              `${t('remaining')}: ${remaining !== null ? formatBytes(remaining) : t('unlimited')}`
            }
          >
            <div>
              <Progress
                percent={pct}
                size="small"
                status={overQuota ? 'exception' : 'normal'}
              />
              <span style={{ fontSize: 11 }}>
                {formatBytes(used)}
                {' / '}
                {unlimited ? t('unlimited') : formatBytes(limit)}
                {overQuota && <Tag color="red" style={{ marginLeft: 4 }}>{t('overQuota')}</Tag>}
              </span>
            </div>
          </Tooltip>
        );
      },
    },
    { title: t('joined'), dataIndex: 'created_at', key: 'created_at' },
    {
      // v0.4.20: standalone "Rule Management" column for admin user-rule management.
      title: t('manageRulesColumn'), key: 'manageRules', width: 70,
      render: (_: unknown, u: User) => (
        <Button icon={<ApiOutlined />} size="small" type="text" onClick={() => navigate(`/rules?owner_uid=${u.id}`)}>{t('manageRules')}</Button>
      ),
    },
    {
      title: t('action'), key: 'action', width: 210,
      render: (_: unknown, u: User) => (
        <Space size="small">
          <Button icon={<EditOutlined />} size="small" type="text" onClick={() => openEdit(u)}>{t('edit')}</Button>
          <Popconfirm title={t('resetTrafficConfirm')} onConfirm={() => handleResetTraffic(u.id)}>
            <Button icon={<UndoOutlined />} size="small" type="text">{t('resetTraffic')}</Button>
          </Popconfirm>
          {/* v0.4.10 PR4: reset password — only for non-admin users (an admin
              changes their own password via /account, never another admin's). */}
          {isAdmin && !u.admin && (
            <Button icon={<KeyOutlined />} size="small" type="text" onClick={() => openReset(u)}>{t('resetPassword')}</Button>
          )}
          {isAdmin && !u.admin && (
            <Popconfirm title={t('deleteUserConfirm')} onConfirm={() => handleDelete(u.id)}>
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
        <h2 className="rp-page-title"><UserOutlined /> {t('users')}</h2>
        <Space>
          {isAdmin && (
            <Button type="primary" icon={<PlusOutlined />} onClick={openCreate}>{t('addUser')}</Button>
          )}
          <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
        </Space>
      </div>
      <Table dataSource={users} columns={columns} rowKey="id" loading={loading} pagination={{ pageSize: 20 }} />

      <Modal
        title={editing ? `${t('editUser')}: ${editing.username}` : t('editUser')}
        open={!!editing}
        confirmLoading={saving}
        onOk={handleSave}
        onCancel={() => setEditing(null)}
        okText={t('save')}
        cancelText={t('cancel')}
      >
        <Form form={form} layout="vertical">
          <Form.Item
            name="balance"
            label={t('balance')}
            tooltip={t('balanceHint')}
            // Rules mirror the backend `parse_balance` checks in
            // crates/shared/src/money.rs. Anything that slips past the form
            // will be rejected by the backend as a 400 — the form check just
            // gives a friendlier inline message before the round-trip.
            rules={[
              { required: true, message: t('balanceRequired') },
              {
                pattern: /^\d+(\.\d{1,2})?$/,
                message: t('balanceInvalid'),
              },
              {
                validator: (_rule, value: string | null | undefined) => {
                  if (!value) return Promise.resolve();
                  // Same cap the backend enforces (9 999 999 999.99).
                  if (value.length > 14 || Number(value) > 9999999999.99) {
                    return Promise.reject(new Error(t('balanceTooLarge')));
                  }
                  return Promise.resolve();
                },
              },
            ]}
          >
            {/*
              stringMode keeps the wire format identical to the DB TEXT
              column and matches the backend parser. precision=2 matches the
              backend's "at most 2 fraction digits" rule.
            */}
            <InputNumber
              stringMode
              min={0}
              max={9999999999.99}
              step={0.01}
              precision={2}
              style={{ width: '100%' }}
              addonBefore={t('balanceUnit')}
              placeholder="0.00"
            />
          </Form.Item>
          <Form.Item
            name="max_rules"
            label={t('maxRules')}
            rules={[{ type: 'number', min: 0, max: 100000, message: t('maxRulesRange') }]}
          >
            <InputNumber min={0} max={100000} style={{ width: '100%' }} />
          </Form.Item>
          <Form.Item
            name="traffic_limit_gb"
            label={t('trafficLimitGb')}
            tooltip={t('trafficLimitGbHint')}
            rules={[{ type: 'number', min: 0, message: t('trafficLimitNonNegative') }]}
          >
            <InputNumber min={0} step={1} style={{ width: '100%' }} addonAfter="GB" />
          </Form.Item>
          <Form.Item name="banned" label={t('banned')} valuePropName="checked">
            <Switch disabled={!!editing?.admin} />
          </Form.Item>
          {!editing?.admin && (
            <Form.Item name="group_id" label={t('userGroups')}>
              <Select allowClear placeholder={t('selectUserGroup')} style={{ width: '100%' }}>
                {userGroups.map(g => <Select.Option key={g.id} value={g.id}>{g.name} (#{g.id})</Select.Option>)}
              </Select>
            </Form.Item>
          )}
        </Form>
      </Modal>

      <Modal
        title={t('addUser')}
        open={creating}
        confirmLoading={saving}
        onOk={handleCreate}
        onCancel={() => setCreating(false)}
        okText={t('create')}
        cancelText={t('cancel')}
      >
        <Form form={createForm} layout="vertical">
          <Form.Item
            name="username"
            label={t('username')}
            tooltip={t('createUsernameHint')}
            rules={[
              { required: true, message: t('createUsernameRequired') },
              {
                pattern: /^[A-Za-z0-9_]{1,64}$/,
                message: t('createUsernameInvalid'),
              },
            ]}
          >
            <Input autoComplete="off" placeholder="username" />
          </Form.Item>
          <Form.Item
            name="password"
            label={t('password')}
            tooltip={t('createPasswordHint')}
            rules={[
              { required: true, message: t('createPasswordRequired') },
              { min: 6, message: t('createPasswordTooShort') },
            ]}
          >
            <Input.Password autoComplete="new-password" placeholder="••••••" />
          </Form.Item>
          <p style={{ color: '#888', fontSize: 12, margin: 0 }}>{t('createUserRoleNote')}</p>
        </Form>
      </Modal>

      {/* v0.4.10 PR4: admin password reset modal. */}
      <Modal
        title={resetting ? `${t('resetPassword')}: ${resetting.username}` : t('resetPassword')}
        open={!!resetting}
        confirmLoading={saving}
        onOk={handleReset}
        onCancel={() => setResetting(null)}
        okText={t('confirmReset')}
        cancelText={t('cancel')}
        okButtonProps={{ danger: true }}
      >
        <p style={{ color: '#cf1322', fontSize: 13, marginTop: 0 }}>
          {t('resetPasswordWarning')}
        </p>
        <Form form={resetForm} layout="vertical">
          <Form.Item
            name="new_password"
            label={t('temporaryPassword')}
            rules={[
              { required: true, message: t('passwordRequired') },
              {
                validator: (_, value: string) => {
                  if (!value) return Promise.resolve();
                  // UTF-8 byte length, matching the backend's 8..=72 bcrypt bound.
                  const bytes = new TextEncoder().encode(value).length;
                  if (bytes < 8) return Promise.reject(new Error(t('passwordTooShort')));
                  if (bytes > 72) return Promise.reject(new Error(t('passwordTooLong')));
                  return Promise.resolve();
                },
              },
            ]}
          >
            <Input.Password autoComplete="new-password" placeholder="••••••••" />
          </Form.Item>
          <Form.Item
            name="confirm_password"
            label={t('confirmPassword')}
            dependencies={['new_password']}
            rules={[
              { required: true, message: t('confirmPasswordRequired') },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error(t('passwordsDoNotMatch')));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="must_change_password"
            label={t('mustChangePasswordNext')}
            valuePropName="checked"
          >
            <Switch />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
