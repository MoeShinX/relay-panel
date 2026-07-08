import { Form, Input, Button, Card, message, Typography, Alert } from 'antd';
import { LockOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope } from '../api/client';
import { useI18n } from '../i18n/context';
import { useAuth } from '../auth/useAuth';
import { makePasswordValidator } from '../utils/password';

const { Title, Text } = Typography;

/** v0.4.10 PR4: forced password change. Reached when the logged-in user has
 *  must_change_password=true (admin reset with a temporary password). Until
 *  they change it the backend returns 403 PASSWORD_CHANGE_REQUIRED for every
 *  endpoint except GET /user/me and PUT /user/password.
 *
 *  On success the backend has bumped token_version, so the current token is
 *  now invalid — we logout + redirect to /login so the user re-authenticates
 *  with the new password (matches the roadmap: "clear old token, require
 *  re-login").
 *
 *  Password length is validated by UTF-8 BYTE count (TextEncoder), matching
 *  the backend's 8..=72 bcrypt boundary. */
export default function ForcePasswordChange() {
  const navigate = useNavigate();
  const { t } = useI18n();
  const { logout } = useAuth();
  const [submitting, setSubmitting] = useState(false);

  const onFinish = async (values: { current_password: string; new_password: string }) => {
    setSubmitting(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>('/user/password', {
        current_password: values.current_password,
        new_password: values.new_password,
      });
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      message.success(t('passwordChanged'));
      // token_version was bumped server-side → current token is dead. Force a
      // clean re-login with the new password.
      logout();
      navigate('/login', { replace: true });
    } catch {
      message.error(t('passwordChangeFailed'));
    } finally {
      setSubmitting(false);
    }
  };

  return (
    <div style={{
      display: 'flex', justifyContent: 'center', alignItems: 'center',
      minHeight: '100vh', background: 'var(--rp-bg)',
    }}>
      <Card style={{ width: 400, boxShadow: 'var(--rp-shadow)' }}>
        <div style={{ textAlign: 'center', marginBottom: 20 }}>
          <Title level={4} style={{ margin: 0 }}>{t('forcePasswordChange')}</Title>
        </div>
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16, fontSize: 13 }}
          message={t('forcePasswordChangeDesc')}
        />
        <Form onFinish={onFinish} layout="vertical">
          <Form.Item
            name="current_password"
            label={t('currentPassword')}
            rules={[{ required: true }]}
          >
            <Input.Password
              prefix={<LockOutlined style={{ color: 'var(--rp-text-tertiary)' }} />}
              autoComplete="current-password"
            />
          </Form.Item>
          <Form.Item
            name="new_password"
            label={t('newPassword')}
            rules={[
              { required: true },
              { validator: makePasswordValidator(t('passwordTooShort'), t('passwordTooLong')) },
            ]}
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
          <Form.Item style={{ marginBottom: 0 }}>
            <Button type="primary" htmlType="submit" block loading={submitting}>
              {t('changePassword')}
            </Button>
          </Form.Item>
        </Form>
        <div style={{ marginTop: 12, textAlign: 'center' }}>
          <Button type="link" size="small" onClick={() => { logout(); navigate('/login'); }}>
            <Text type="secondary" style={{ fontSize: 12 }}>{t('logout')}</Text>
          </Button>
        </div>
      </Card>
    </div>
  );
}
