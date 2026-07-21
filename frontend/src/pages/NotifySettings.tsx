import { Card, Form, Switch, Input, InputNumber, Button, Space, message, Spin, Alert, Typography, Divider } from 'antd';
import { SendOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, NotifyConfigPublic, TestNotifyResult } from '../api/types';
import { MIN_OFFLINE_ALERT_SECS } from '../api/types';
import { useI18n } from '../i18n/context';

const { Text } = Typography;

/**
 * v1.2.0: node-offline notification settings.
 *
 * Its own card + form, separate from the registration settings above it, so
 * saving one section can never submit the other.
 */
export default function NotifySettings() {
  const { t } = useI18n();
  const [form] = Form.useForm();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState<string | null>(null);
  const [cfg, setCfg] = useState<NotifyConfigPublic | null>(null);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.get<unknown, ApiEnvelope<NotifyConfigPublic>>('/admin/settings/notify');
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('settingsLoadFailed'));
        return;
      }
      setCfg(res.data);
      // Credential fields stay EMPTY: the API never sends them, and an empty
      // submit means "keep the stored one". The placeholder tells the user one
      // is already configured.
      form.setFieldsValue({ ...res.data, telegram_bot_token: '', smtp_password: '' });
    } catch {
      message.error(t('settingsLoadFailed'));
    } finally {
      setLoading(false);
    }
  }, [form, t]);

  useEffect(() => { load(); }, [load]);

  /** Persist the form. Returns true on success so `onTest` can chain off it. */
  const save = async (silent = false): Promise<boolean> => {
    let values;
    try {
      values = await form.validateFields();
    } catch {
      return false; // antd already highlighted the offending field
    }
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<NotifyConfigPublic>>(
        '/admin/settings/notify',
        {
          ...values,
          // Empty string = "unchanged" on the backend. Sending undefined would
          // be equivalent, but an explicit empty keeps the payload shape fixed.
          telegram_bot_token: values.telegram_bot_token || '',
          smtp_password: values.smtp_password || '',
        },
      );
      if (res.code !== 0) { message.error(res.message); return false; }
      setCfg(res.data ?? null);
      // Re-blank the credential inputs so a second save doesn't resend what the
      // user typed once (and so the placeholder flips to "configured").
      form.setFieldsValue({ telegram_bot_token: '', smtp_password: '' });
      if (!silent) message.success(t('settingsSaved'));
      return true;
    } catch {
      message.error(t('settingsSaveFailed'));
      return false;
    } finally {
      setSaving(false);
    }
  };

  /**
   * Send a real test message.
   *
   * The backend tests the STORED config, so this saves first — otherwise
   * someone types a token, clicks "test", and unknowingly exercises the OLD
   * config. Saving first makes the button do what it visibly promises.
   */
  const onTest = async (channel: 'telegram' | 'email') => {
    if (!(await save(true))) return;
    setTesting(channel);
    try {
      const res = await api.post<unknown, ApiEnvelope<TestNotifyResult>>(
        '/admin/settings/notify/test',
        { channel },
      );
      if (res.code !== 0) { message.error(res.message); return; }
      if (res.data?.ok) {
        message.success(t('notifyTestSent'));
      } else {
        // Show the provider's own words ("chat not found", auth failure) — a
        // generic "failed" would leave the operator with nothing to act on.
        message.error(`${t('notifyTestFailed')}: ${res.data?.detail ?? ''}`, 8);
      }
    } catch {
      message.error(t('notifyTestFailed'));
    } finally {
      setTesting(null);
    }
  };

  // The Form is rendered even while loading rather than returning early: an
  // unattached `useForm` instance is both an antd warning AND a real bug —
  // `load()` calls setFieldsValue, and on a form that isn't mounted yet those
  // values silently don't stick.
  return (
    <Card
      title={t('notifySettings')}
      style={{ marginTop: 16 }}
      extra={
        <Button type="primary" loading={saving} disabled={loading} onClick={() => save()}>
          {t('save')}
        </Button>
      }
    >
      <Alert
        type="info"
        showIcon
        style={{ marginBottom: 16 }}
        title={t('notifyIntro')}
        description={t('notifyIntroDesc')}
      />

      <Spin spinning={loading}>
      <Form form={form} layout="vertical">
        <Form.Item name="enabled" label={t('notifyEnabled')} valuePropName="checked">
          <Switch />
        </Form.Item>

        <Form.Item
          name="offline_alert_secs"
          label={t('offlineAlertSecs')}
          extra={t('offlineAlertSecsHint').replace('{min}', String(MIN_OFFLINE_ALERT_SECS))}
          rules={[{
            validator: (_, v) => (Number(v) >= MIN_OFFLINE_ALERT_SECS
              ? Promise.resolve()
              : Promise.reject(new Error(
                t('offlineAlertSecsTooSmall').replace('{min}', String(MIN_OFFLINE_ALERT_SECS)),
              ))),
          }]}
        >
          <InputNumber min={MIN_OFFLINE_ALERT_SECS} style={{ width: '100%' }} addonAfter={t('seconds')} />
        </Form.Item>

        <Form.Item name="notify_recovery" label={t('notifyRecovery')} extra={t('notifyRecoveryHint')} valuePropName="checked">
          <Switch />
        </Form.Item>

        <Divider>Telegram</Divider>

        <Form.Item name="telegram_enabled" label={t('enableChannel')} valuePropName="checked">
          <Switch />
        </Form.Item>
        <Form.Item name="telegram_bot_token" label="Bot Token" extra={t('credentialKeepHint')}>
          <Input.Password
            autoComplete="off"
            placeholder={cfg?.telegram_bot_token_set ? t('credentialConfigured') : t('credentialEmpty')}
          />
        </Form.Item>
        <Form.Item name="telegram_chat_id" label="Chat ID" extra={t('telegramChatIdHint')}>
          <Input autoComplete="off" placeholder="-1001234567890" />
        </Form.Item>
        <Form.Item>
          <Button
            icon={<SendOutlined />}
            loading={testing === 'telegram'}
            onClick={() => onTest('telegram')}
          >
            {t('saveAndTest')}
          </Button>
        </Form.Item>

        <Divider>{t('email')}</Divider>

        <Form.Item name="email_enabled" label={t('enableChannel')} valuePropName="checked">
          <Switch />
        </Form.Item>
        <Form.Item name="smtp_host" label={t('smtpHost')}>
          <Input autoComplete="off" placeholder="smtp.example.com" />
        </Form.Item>
        <Form.Item name="smtp_port" label={t('smtpPort')} extra={t('smtpPortHint')}>
          <InputNumber min={1} max={65535} style={{ width: '100%' }} placeholder="465" />
        </Form.Item>
        <Form.Item name="smtp_tls" label={t('smtpTls')} extra={t('smtpTlsHint')} valuePropName="checked">
          <Switch />
        </Form.Item>
        <Form.Item name="smtp_username" label={t('smtpUsername')}>
          <Input autoComplete="off" placeholder="ops@example.com" />
        </Form.Item>
        <Form.Item name="smtp_password" label={t('smtpPassword')} extra={t('credentialKeepHint')}>
          <Input.Password
            autoComplete="new-password"
            placeholder={cfg?.smtp_password_set ? t('credentialConfigured') : t('credentialEmpty')}
          />
        </Form.Item>
        <Form.Item name="smtp_from" label={t('smtpFrom')} extra={t('smtpFromHint')}>
          <Input autoComplete="off" placeholder="ops@example.com" />
        </Form.Item>
        <Form.Item name="smtp_to" label={t('smtpTo')} extra={t('smtpToHint')}>
          <Input autoComplete="off" placeholder="admin@example.com" />
        </Form.Item>
        <Form.Item>
          <Space>
            <Button
              icon={<SendOutlined />}
              loading={testing === 'email'}
              onClick={() => onTest('email')}
            >
              {t('saveAndTest')}
            </Button>
          </Space>
        </Form.Item>

        <Text type="secondary" style={{ fontSize: 12 }}>{t('notifyCredentialNote')}</Text>
      </Form>
      </Spin>
    </Card>
  );
}
