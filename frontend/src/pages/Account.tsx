import { useEffect, useState } from 'react';
import { Card, Descriptions, Spin, Button, Space, Modal, Form, Input, message, Typography, Result, Progress, Alert, Tag } from 'antd';
import { LockOutlined, LogoutOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, UserSelf } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';
import { useAuth } from '../auth/useAuth';

const { Text } = Typography;

/**
 * v0.4.9: a regular user's own account page (GET /user/me). This is the
 * non-admin landing page after login — the Dashboard is admin-only (it hits
 * /admin/* endpoints), so a non-admin needs a page that only reads their own
 * row. Admins can also view it (it's linked in the menu for everyone).
 *
 * v0.4.10: expanded to show plan + current/limit rule count. Logout delegates
 * to AuthContext.logout (single source of truth). The page still loads /user/me
 * itself (rather than only reading AuthContext.user) so it has its own
 * loading/error/retry UI independent of the app-wide boot state.
 *
 * Shows: username, plan, balance, rules (current/limit), traffic used / quota,
 * member-since. Change-password reuses the existing self-service PUT
 * /user/password (an AuthUser endpoint, not admin).
 */
export default function Account() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const { logout: authLogout } = useAuth();
  const [me, setMe] = useState<UserSelf | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);

  // Change-password modal state (mirrors MainLayout's modal).
  const [changePwOpen, setChangePwOpen] = useState(false);
  const [pwForm] = Form.useForm();
  const [pwSubmitting, setPwSubmitting] = useState(false);

  const load = async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const res = await api.get<unknown, ApiEnvelope<UserSelf>>('/user/me');
      if (res.code !== 0 || !res.data) {
        setLoadFailed(true);
        return;
      }
      setMe(res.data);
    } catch {
      // 403/404/network — show a retry result rather than flashing to login.
      // (401 is handled by the global interceptor; reaching here means the
      // request failed for another reason.)
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => { load(); }, []);

  const logout = () => {
    authLogout();
    navigate('/login');
  };

  const handleChangePassword = async (values: { current_password: string; new_password: string }) => {
    setPwSubmitting(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>('/user/password', values);
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      message.success(t('passwordChanged'));
      setChangePwOpen(false);
      pwForm.resetFields();
    } catch {
      message.error(t('passwordChangeFailed'));
    } finally {
      setPwSubmitting(false);
    }
  };

  if (loading) {
    return <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;
  }

  if (loadFailed || !me) {
    return (
      <Result
        status="warning"
        title={t('accountLoadFailed')}
        extra={<Button type="primary" onClick={load}>{t('refresh')}</Button>}
      />
    );
  }

  return (
    <>
      {/* v1.0.8: suspended banner — the user can still log in and buy a plan
          (buying does NOT auto-unsuspend), but forwarding is gated off. */}
      {me.suspended && (
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('accountSuspended')}
          description={t('accountSuspendedHint')}
        />
      )}
      <Card
        title={t('myAccount')}
        extra={
          <Space>
            <Button icon={<LockOutlined />} onClick={() => setChangePwOpen(true)}>{t('changePassword')}</Button>
            <Button danger icon={<LogoutOutlined />} onClick={logout}>{t('logout')}</Button>
          </Space>
        }
      >
        <Descriptions column={1} bordered size="small">
          <Descriptions.Item label={t('accountUsername')}>
            <Text strong>{me.username}</Text>
            {me.admin && <Text type="secondary"> ({t('admin')})</Text>}
          </Descriptions.Item>
          <Descriptions.Item label={t('accountPlan')}>
            {me.plan_name || '-'}
          </Descriptions.Item>
          {/* v1.0.8: plan expiry (null = no expiry). */}
          <Descriptions.Item label={t('accountPlanExpiry')}>
            {me.plan_expire_at ? <span className="rp-mono">{me.plan_expire_at}</span> : t('unlimited')}
          </Descriptions.Item>
          {/* v1.0.8: available lines — "全部" when unrestricted (admin or
              all_device_groups), otherwise the specific authorized group names. */}
          <Descriptions.Item label={t('accountAvailableGroups')}>
            {me.all_groups ? (
              <Tag color="green">{t('allGroups')}</Tag>
            ) : me.available_groups && me.available_groups.length > 0 ? (
              <Space wrap>
                {me.available_groups.map((name) => <Tag key={name}>{name}</Tag>)}
              </Space>
            ) : (
              <Text type="secondary">{t('noneAssigned')}</Text>
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t('accountBalance')}>
            <span className="rp-mono">{me.balance}</span>
          </Descriptions.Item>
          <Descriptions.Item label={t('accountRulesLimit')}>
            {me.current_rules} / {me.max_rules}
          </Descriptions.Item>
          {/* v1.0.8: traffic usage with a progress bar (used / limit). */}
          <Descriptions.Item label={t('accountTrafficUsed')}>
            {me.traffic_limit > 0 ? (
              <Space orientation="vertical" style={{ width: '100%', maxWidth: 360 }}>
                <span>{formatBytes(me.traffic_used)} / {formatBytes(me.traffic_limit)}</span>
                <Progress
                  percent={Math.min(100, Math.round((me.traffic_used / me.traffic_limit) * 100))}
                  size="small"
                  status={me.traffic_used >= me.traffic_limit ? 'exception' : 'active'}
                />
              </Space>
            ) : (
              <span>{formatBytes(me.traffic_used)} / {t('unlimited')}</span>
            )}
          </Descriptions.Item>
          <Descriptions.Item label={t('accountMemberSince')}>
            <span className="rp-mono">{me.registered_at || '-'}</span>
          </Descriptions.Item>
        </Descriptions>
      </Card>

      <Modal
        title={t('changePassword')}
        open={changePwOpen}
        onCancel={() => { setChangePwOpen(false); pwForm.resetFields(); }}
        onOk={() => pwForm.submit()}
        okText={t('save')}
        cancelText={t('cancel')}
        confirmLoading={pwSubmitting}
      >
        <Form form={pwForm} onFinish={handleChangePassword} layout="vertical">
          <Form.Item
            name="current_password"
            label={t('currentPassword')}
            rules={[{ required: true }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item
            name="new_password"
            label={t('newPassword')}
            rules={[{ required: true, min: 6, message: t('newPasswordTooShort') }]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm_password"
            label={t('confirmPassword')}
            dependencies={['new_password']}
            rules={[
              { required: true },
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
        </Form>
      </Modal>
    </>
  );
}
