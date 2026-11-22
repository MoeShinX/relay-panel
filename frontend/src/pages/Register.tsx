import { Form, Input, Button, Card, message, Typography, Segmented, Result, Spin, Select } from 'antd';
import { UserOutlined, LockOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, RegistrationStatus, Plan } from '../api/types';
import { useI18n } from '../i18n/context';
import { useSite } from '../hooks/useSite';
import { makePasswordValidator } from '../utils/password';

const { Title, Text } = Typography;

/** v0.4.10 PR3 / v0.4.21 PR2: self-service registration page.
 *
 *  v0.4.21 PR2: when the admin has enabled multiple plans for registration,
 *  a plan selector is shown; otherwise the single allowed plan is used
 *  automatically. plan_id is submitted with the registration request. */
export default function Register() {
  const navigate = useNavigate();
  const { t, lang, setLang } = useI18n();
  const site = useSite();
  const [status, setStatus] = useState<RegistrationStatus | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [submitting, setSubmitting] = useState(false);
  const [selectedPlanId, setSelectedPlanId] = useState<number | undefined>(undefined);

  const checkStatus = async () => {
    setLoadFailed(false);
    try {
      const res = await api.get<unknown, ApiEnvelope<RegistrationStatus>>(
        '/auth/registration-status'
      );
      const data = res.data ?? { enabled: false, default_plan_id: 1, plans: [], default_password_change_required: false };
      setStatus(data);
      // Pre-select the default plan.
      setSelectedPlanId(data.default_plan_id);
    } catch {
      setLoadFailed(true);
    }
  };

  useEffect(() => {
    checkStatus();
  }, []);

  // Redirect to /login once we know registration is closed.
  useEffect(() => {
    if (status && !status.enabled) {
      message.info(t('registrationClosed'));
      navigate('/login', { replace: true });
    }
  }, [status, navigate, t]);

  const onFinish = async (values: { username: string; password: string }) => {
    setSubmitting(true);
    try {
      const body: Record<string, unknown> = {
        username: values.username,
        password: values.password,
      };
      // Include plan_id if the user made a selection (or the default was set).
      if (selectedPlanId != null) {
        body.plan_id = selectedPlanId;
      }
      const res = await api.post<unknown, ApiEnvelope<null>>('/auth/register', body);
      if (res.code !== 0) {
        message.error(res.message || t('registerFailed'));
        return;
      }
      message.success(t('registerSuccess'));
      navigate('/login');
    } catch {
      message.error(t('registerFailed'));
    } finally {
      setSubmitting(false);
    }
  };

  const plans: Plan[] = status?.plans ?? [];

  return (
    <div className="rp-auth-shell">
      <div style={{ position: 'absolute', top: 'max(20px, env(safe-area-inset-top))', right: 24 }}>
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
      <Card className="rp-auth-card">
        <div style={{ textAlign: 'center', marginBottom: 28 }}>
          <div style={{ margin: '0 auto 14px', width: 44, height: 44, display: 'flex', alignItems: 'center', justifyContent: 'center' }}>
            <svg aria-hidden="true" width="44" height="44" viewBox="0 0 64 64" fill="none" style={{ filter: 'drop-shadow(0 6px 14px rgba(79, 70, 229, 0.22))' }}>
              <defs>
                <linearGradient id="rp-reg-logo" x1="8" y1="5" x2="57" y2="61" gradientUnits="userSpaceOnUse">
                  <stop stopColor="#818CF8" />
                  <stop offset="1" stopColor="#4F46E5" />
                </linearGradient>
              </defs>
              <rect x="3" y="3" width="58" height="58" rx="16" fill="url(#rp-reg-logo)" />
              <path d="M15 23h27m0 0-7-7m7 7-7 7M49 41H22m0 0 7 7m-7-7 7-7" stroke="#FFFFFF" strokeWidth="6" strokeLinecap="round" strokeLinejoin="round" />
            </svg>
          </div>
          <Title level={3} style={{ margin: '0 0 4px', fontWeight: 600, letterSpacing: -0.3 }}>
            {site.site_name ? `${site.site_name} · ${t('registerTitle')}` : t('registerTitle')}
          </Title>
          <Text type="secondary" style={{ fontSize: 13 }}>{site.subtitle || t('subtitle')}</Text>
        </div>

        {loadFailed ? (
          <Result
            status="warning"
            title={t('registrationStatusFailed')}
            extra={<Button type="primary" onClick={checkStatus}>{t('retry')}</Button>}
          />
        ) : status === null ? (
          <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>
        ) : (
          <Form onFinish={onFinish} size="large">
            <Form.Item
              name="username"
              rules={[{ required: true, message: t('usernameRequired') }]}
            >
              <Input
                prefix={<UserOutlined style={{ color: 'var(--rp-text-tertiary)' }} />}
                placeholder={t('username')}
                aria-label={t('username')}
                autoComplete="username"
                spellCheck={false}
              />
            </Form.Item>
            <Form.Item
              name="password"
              rules={[
                { required: true, message: t('passwordRequired') },
                { validator: makePasswordValidator(t('passwordTooShort'), t('passwordTooLong')) },
              ]}
            >
              <Input.Password
                prefix={<LockOutlined style={{ color: 'var(--rp-text-tertiary)' }} />}
                placeholder={t('password')}
                aria-label={t('password')}
                autoComplete="new-password"
              />
            </Form.Item>
            <Form.Item
              name="confirm_password"
              dependencies={['password']}
              rules={[
                { required: true, message: t('confirmPasswordRequired') },
                ({ getFieldValue }) => ({
                  validator(_, value) {
                    if (!value || getFieldValue('password') === value) {
                      return Promise.resolve();
                    }
                    return Promise.reject(new Error(t('passwordsDoNotMatch')));
                  },
                }),
              ]}
            >
              <Input.Password
                prefix={<LockOutlined style={{ color: 'var(--rp-text-tertiary)' }} />}
                placeholder={t('confirmPassword')}
                autoComplete="new-password"
                aria-label={t('confirmPassword')}
              />
            </Form.Item>

            {/* v0.4.21 PR2: plan selector — shown when multiple plans are available. */}
            {plans.length > 1 && (
              <Form.Item label={t('selectPlan')}>
                <Select
                  value={selectedPlanId}
                  onChange={(v) => setSelectedPlanId(v)}
                  options={plans.map(p => ({
                    value: p.id,
                    label: `${p.name} (${p.max_rules} ${t('rules')})`,
                  }))}
                />
              </Form.Item>
            )}

            <Form.Item style={{ marginBottom: 12 }}>
              <Button type="primary" htmlType="submit" block loading={submitting}>
                {t('register')}
              </Button>
            </Form.Item>
          </Form>
        )}
        <div style={{ marginTop: 4, textAlign: 'center' }}>
          <Button type="link" size="small" style={{ padding: 0 }} onClick={() => navigate('/login')}>
            {t('backToLogin')}
          </Button>
        </div>
      </Card>
    </div>
  );
}
