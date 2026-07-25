import { Card, Form, Switch, Input, InputNumber, Button, message, Spin, Alert, Typography, Row, Col } from 'antd';
import { SendOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, NotifyConfigPublic, TestNotifyResult } from '../api/types';
import { MIN_OFFLINE_ALERT_SECS } from '../api/types';
import { useI18n } from '../i18n/context';

const { Text } = Typography;

/**
 * A boolean row: label + hint on the left, switch on the right.
 *
 * The default vertical Form.Item puts the label above the control, so every
 * toggle cost three stacked lines for one bit of state. Pairing them on one
 * line is both shorter and easier to scan — the switches line up in a column
 * you can read down.
 */
function SwitchRow({ name, label, hint }: { name: string; label: string; hint?: string }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 16, padding: '6px 0' }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div>{label}</div>
        {hint && (
          <div style={{ color: 'var(--rp-text-tertiary)', fontSize: 12, marginTop: 2 }}>{hint}</div>
        )}
      </div>
      <Form.Item name={name} valuePropName="checked" noStyle>
        <Switch />
      </Form.Item>
    </div>
  );
}

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
      {/* Laid out in columns rather than one long stack of full-width inputs.
          A chat id or a port is a short value; giving each its own full-width
          row made the page scroll for no reason. Everything collapses to a
          single column below `lg`. */}
      <Form form={form} layout="vertical">
        <Row gutter={16}>
        <Col xs={24} lg={12}>
        <Card size="small" style={{ marginBottom: 16 }} styles={{ body: { minHeight: 168 } }}>
          <SwitchRow name="enabled" label={t('notifyEnabled')} />
          <Form.Item
            name="offline_alert_secs"
            label={t('offlineAlertSecs')}
            extra={t('offlineAlertSecsHint').replace('{min}', String(MIN_OFFLINE_ALERT_SECS))}
            style={{ marginTop: 12, marginBottom: 12 }}
            rules={[{
              validator: (_, v) => (Number(v) >= MIN_OFFLINE_ALERT_SECS
                ? Promise.resolve()
                : Promise.reject(new Error(
                  t('offlineAlertSecsTooSmall').replace('{min}', String(MIN_OFFLINE_ALERT_SECS)),
                ))),
            }]}
          >
            <InputNumber min={MIN_OFFLINE_ALERT_SECS} style={{ width: 180 }} addonAfter={t('seconds')} />
          </Form.Item>
          <SwitchRow name="notify_recovery" label={t('notifyRecovery')} hint={t('notifyRecoveryHint')} />
        </Card>
        </Col>

        {/* Telegram sits beside the global block, top-aligned with it: it only
            has two fields, so on its own row it wasted most of the width. */}
        <Col xs={24} lg={12}>

        <Card
          size="small"
          title="Telegram"
          /* The channel's on/off lives in the header rather than as a labelled
             row inside: it governs the whole card, and putting it there removes
             a full field from each channel. */
          extra={
            <Form.Item name="telegram_enabled" valuePropName="checked" noStyle>
              <Switch size="small" />
            </Form.Item>
          }
          style={{ marginBottom: 16 }}
        >
          <Row gutter={16}>
            <Col xs={24} md={12}>
              <Form.Item name="telegram_bot_token" label="Bot Token" extra={t('credentialKeepHint')}>
                <Input.Password
                  autoComplete="off"
                  placeholder={cfg?.telegram_bot_token_set ? t('credentialConfigured') : t('credentialEmpty')}
                />
              </Form.Item>
            </Col>
            <Col xs={24} md={12}>
              <Form.Item name="telegram_chat_id" label="Chat ID" extra={t('telegramChatIdHint')}>
                <Input autoComplete="off" placeholder="-1001234567890" />
              </Form.Item>
            </Col>
          </Row>
          <Button
            icon={<SendOutlined />}
            loading={testing === 'telegram'}
            onClick={() => onTest('telegram')}
          >
            {t('saveAndTest')}
          </Button>
        </Card>
        </Col>
        </Row>

        <Card
          size="small"
          title={t('email')}
          extra={
            <Form.Item name="email_enabled" valuePropName="checked" noStyle>
              <Switch size="small" />
            </Form.Item>
          }
          style={{ marginBottom: 16 }}
        >
          {/* Full width, so the seven fields fit in two dense rows instead of
              seven stacked ones. Grouped by what you set together: the server
              (host/port/TLS) on one line, then credentials and addresses. */}
          <Row gutter={16}>
            <Col xs={24} md={12} lg={8}>
              <Form.Item name="smtp_host" label={t('smtpHost')}>
                <Input autoComplete="off" placeholder="smtp.example.com" />
              </Form.Item>
            </Col>
            <Col xs={12} md={6} lg={4}>
              <Form.Item name="smtp_port" label={t('smtpPort')} extra={t('smtpPortHint')}>
                <InputNumber min={1} max={65535} style={{ width: '100%' }} placeholder="465" />
              </Form.Item>
            </Col>
            <Col xs={12} md={6} lg={4}>
              <Form.Item name="smtp_tls" label={t('smtpTls')} extra={t('smtpTlsHint')} valuePropName="checked">
                <Switch />
              </Form.Item>
            </Col>
            <Col xs={24} md={12} lg={8}>
              <Form.Item name="smtp_username" label={t('smtpUsername')}>
                <Input autoComplete="off" placeholder="ops@example.com" />
              </Form.Item>
            </Col>
          </Row>
          <Row gutter={16}>
            <Col xs={24} md={8}>
              <Form.Item name="smtp_password" label={t('smtpPassword')} extra={t('credentialKeepHint')}>
                <Input.Password
                  autoComplete="new-password"
                  placeholder={cfg?.smtp_password_set ? t('credentialConfigured') : t('credentialEmpty')}
                />
              </Form.Item>
            </Col>
            <Col xs={24} md={8}>
              <Form.Item name="smtp_from" label={t('smtpFrom')} extra={t('smtpFromHint')}>
                <Input autoComplete="off" placeholder="ops@example.com" />
              </Form.Item>
            </Col>
            <Col xs={24} md={8}>
              <Form.Item name="smtp_to" label={t('smtpTo')} extra={t('smtpToHint')}>
                <Input autoComplete="off" placeholder="admin@example.com" />
              </Form.Item>
            </Col>
          </Row>
          <Button
            icon={<SendOutlined />}
            loading={testing === 'email'}
            onClick={() => onTest('email')}
          >
            {t('saveAndTest')}
          </Button>
        </Card>

        <Text type="secondary" style={{ fontSize: 12 }}>{t('notifyCredentialNote')}</Text>
      </Form>
      </Spin>
    </Card>
  );
}
