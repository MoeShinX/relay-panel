import { Form, Input, Button, Card, message, Typography, Segmented, Alert } from 'antd';
import { UserOutlined, LockOutlined, InfoCircleOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, LoginResponse, RegistrationStatus } from '../api/types';
import { useI18n } from '../i18n/context';
import { useAuth } from '../auth/useAuth';

const { Title, Text } = Typography;

export default function Login() {
  const navigate = useNavigate();
  const { t, lang, setLang } = useI18n();
  const { login } = useAuth();
  // v0.4.10 PR3: whether to show the "create account" link. null = still
  // loading (don't flash the link then remove it); a network failure leaves
  // it hidden rather than guessing.
  const [regEnabled, setRegEnabled] = useState<boolean | null>(null);
  // v0.4.22: whether to show the "change default password" security banner.
  // Driven by the server's must_change_password flag on the admin account.
  const [showPwdWarning, setShowPwdWarning] = useState(false);

  useEffect(() => {
    api
      .get<unknown, ApiEnvelope<RegistrationStatus>>('/auth/registration-status')
      .then((res) => {
        setRegEnabled(res.data?.enabled === true);
        setShowPwdWarning(res.data?.default_password_change_required === true);
      })
      .catch(() => setRegEnabled(false));
  }, []);

  const onFinish = async (values: { username: string; password: string }) => {
    try {
      const res = await api.post<unknown, ApiEnvelope<LoginResponse>>('/auth/login', values);
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('loginFailedMsg'));
        return;
      }
      // v0.4.10: login() persists the token then fetches /user/me so the role
      // comes from the server (not the login response, which could diverge).
      await login(res.data.token);
      message.success(t('loginSuccess'));
      // Both roles land on / — RoleHome renders Dashboard (admin) or
      // redirects regular users to /account (the regular dashboard was removed).
      navigate('/');
    } catch {
      message.error(t('loginFailed'));
    }
  };

  return (
    <div style={{
      display: 'flex', justifyContent: 'center', alignItems: 'center',
      minHeight: '100vh', background: 'var(--rp-bg)',
    }}>
      <div style={{ position: 'absolute', top: 20, right: 24 }}>
        <Segmented
          size="small"
          value={lang}
          onChange={(v) => setLang(v as 'zh-CN' | 'en-US')}
          options={[
            { value: 'zh-CN', label: t('langZhCN') },
            { value: 'en-US', label: t('langEnUS') },
          ]}
        />
      </div>
      <Card style={{ width: 380, boxShadow: 'var(--rp-shadow)' }}>
        <div style={{ textAlign: 'center', marginBottom: 28 }}>
          <Title level={3} style={{ margin: 0, fontWeight: 600 }}>{t('brand')}</Title>
          <Text type="secondary" style={{ fontSize: 13 }}>{t('subtitle')}</Text>
        </div>

        {/* v0.3.6 / v0.4.22: first-login security reminder. Only shown when
            the admin account still has must_change_password set (driven by
            the /auth/registration-status response). */}
        {showPwdWarning && (
          <Alert
            icon={<InfoCircleOutlined />}
            type="warning"
            showIcon
            style={{ marginBottom: 16, fontSize: 12 }}
            title={t('changeDefaultPasswordWarning')}
          />
        )}

        <Form onFinish={onFinish} size="large">
          <Form.Item name="username" rules={[{ required: true, message: t('usernameRequired') }]}>
            <Input prefix={<UserOutlined style={{ color: 'var(--rp-text-tertiary)' }} />} placeholder={t('username')} aria-label={t('username')} />
          </Form.Item>
          <Form.Item name="password" rules={[{ required: true, message: t('passwordRequired') }]}>
            <Input.Password prefix={<LockOutlined style={{ color: 'var(--rp-text-tertiary)' }} />} placeholder={t('password')} aria-label={t('password')} />
          </Form.Item>
          <Form.Item style={{ marginBottom: 12 }}>
            <Button type="primary" htmlType="submit" block>{t('login')}</Button>
          </Form.Item>
        </Form>
        {regEnabled && (
          <div style={{ marginTop: 4, textAlign: 'center' }}>
            <Button type="link" size="small" style={{ padding: 0 }} onClick={() => navigate('/register')}>
              {t('createAccount')}
            </Button>
          </div>
        )}
        <div style={{ marginTop: 16, textAlign: 'center' }}>
          <a
            href="https://github.com/MoeShinX/relay-panel"
            target="_blank"
            rel="noopener noreferrer"
            style={{ fontSize: 12, color: 'var(--rp-text-tertiary)' }}
          >
            {t('sourceCode')}
          </a>
        </div>
      </Card>
    </div>
  );
}
