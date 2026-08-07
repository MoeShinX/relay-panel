import { Alert, Button, Card, Col, Empty, Form, Input, InputNumber, message, Row, Space, Spin, Switch, Table, Tag, Typography } from 'antd';
import { SendOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, NotifyConfigPublic, NotifyHistoryEntry, TestNotifyResult } from '../api/types';
import { MIN_OFFLINE_ALERT_SECS } from '../api/types';
import { useI18n } from '../i18n/context';
import type { I18nContextValue } from '../i18n/context';

const { Text } = Typography;

function SwitchRow({ name, label, hint }: { name: string; label: string; hint?: string }) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 16, padding: '8px 0' }}>
      <div style={{ flex: 1, minWidth: 0 }}>
        <div>{label}</div>
        {hint && <div style={{ color: 'var(--rp-text-tertiary)', fontSize: 12, marginTop: 2 }}>{hint}</div>}
      </div>
      <Form.Item name={name} valuePropName="checked" noStyle>
        <Switch aria-label={label} />
      </Form.Item>
    </div>
  );
}

function ChannelStatus({ enabled, configured, t }: { enabled: boolean; configured: boolean; t: I18nContextValue['t'] }) {
  if (!configured) return <Tag>{t('channelNotConfigured')}</Tag>;
  if (enabled) return <Tag color="success">{t('channelConfiguredEnabled')}</Tag>;
  return <Tag color="default">{t('channelConfiguredDisabled')}</Tag>;
}

const eventKey = {
  offline: 'notifyEventOffline',
  offline_reminder: 'notifyEventOfflineReminder',
  recovery: 'notifyEventRecovery',
  version_outdated: 'notifyEventVersionOutdated',
  test: 'notifyEventTest',
} as const;

export default function NotifySettings() {
  const { t } = useI18n();
  const [form] = Form.useForm();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [testing, setTesting] = useState<string | null>(null);
  const [cfg, setCfg] = useState<NotifyConfigPublic | null>(null);
  const [history, setHistory] = useState<NotifyHistoryEntry[]>([]);
  const [historyLoading, setHistoryLoading] = useState(true);

  const telegramEnabled = Form.useWatch('telegram_enabled', form) ?? false;
  const emailEnabled = Form.useWatch('email_enabled', form) ?? false;
  const globallyEnabled = Form.useWatch('enabled', form) ?? false;

  const loadHistory = useCallback(async () => {
    setHistoryLoading(true);
    try {
      const res = await api.get<unknown, ApiEnvelope<NotifyHistoryEntry[]>>('/admin/settings/notify/history');
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('notifyHistoryLoadFailed'));
        return;
      }
      setHistory(res.data);
    } catch {
      message.error(t('notifyHistoryLoadFailed'));
    } finally {
      setHistoryLoading(false);
    }
  }, [t]);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const res = await api.get<unknown, ApiEnvelope<NotifyConfigPublic>>('/admin/settings/notify');
      if (res.code !== 0 || !res.data) {
        message.error(res.message || t('settingsLoadFailed'));
        return;
      }
      setCfg(res.data);
      form.setFieldsValue({ ...res.data, telegram_bot_token: '', smtp_password: '' });
    } catch {
      message.error(t('settingsLoadFailed'));
    } finally {
      setLoading(false);
    }
  }, [form, t]);

  useEffect(() => { void load(); void loadHistory(); }, [load, loadHistory]);

  const save = async (silent = false): Promise<boolean> => {
    let values;
    try {
      values = await form.validateFields();
    } catch {
      return false;
    }
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<NotifyConfigPublic>>('/admin/settings/notify', {
        ...values,
        telegram_bot_token: values.telegram_bot_token || '',
        smtp_password: values.smtp_password || '',
      });
      if (res.code !== 0) { message.error(res.message); return false; }
      setCfg(res.data ?? null);
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

  const onTest = async (channel: 'telegram' | 'email') => {
    if (!(await save(true))) return;
    setTesting(channel);
    try {
      const res = await api.post<unknown, ApiEnvelope<TestNotifyResult>>('/admin/settings/notify/test', { channel });
      if (res.code !== 0) { message.error(res.message); return; }
      if (res.data?.ok) message.success(t('notifyTestSent'));
      else message.error(`${t('notifyTestFailed')}: ${res.data?.detail ?? ''}`, 8);
      await loadHistory();
    } catch {
      message.error(t('notifyTestFailed'));
    } finally {
      setTesting(null);
    }
  };

  const telegramConfigured = Boolean(cfg?.telegram_bot_token_set && cfg.telegram_chat_id);
  const emailConfigured = Boolean(cfg?.smtp_host && cfg.smtp_to);

  const columns = [
    { title: t('notifyHistoryTime'), dataIndex: 'created_at', key: 'created_at', width: 180, render: (v: string) => new Date(v).toLocaleString() },
    { title: t('notifyHistoryEvent'), dataIndex: 'event', key: 'event', width: 140, render: (v: string) => {
      const key = eventKey[v as keyof typeof eventKey];
      return key ? t(key) : v;
    } },
    { title: t('notifyHistoryNode'), dataIndex: 'node_key', key: 'node_key', width: 150, render: (v?: string | null) => v || '-' },
    { title: t('notifyHistoryChannel'), dataIndex: 'channel', key: 'channel', width: 110 },
    { title: t('notifyHistoryResult'), dataIndex: 'status', key: 'status', width: 110, render: (v: string) => <Tag color={v === 'sent' ? 'success' : 'error'}>{v === 'sent' ? t('notifyHistorySent') : t('notifyHistoryFailed')}</Tag> },
    { title: t('notifyHistoryDetail'), dataIndex: 'detail', key: 'detail', render: (v: string) => v || '-' },
  ];

  return (
    <Card title={t('notifySettings')} style={{ marginTop: 16 }} extra={<Button type="primary" loading={saving} disabled={loading} onClick={() => void save()}>{t('save')}</Button>}>
      <Alert type="info" showIcon style={{ marginBottom: 16 }} title={t('notifyIntro')} description={t('notifyIntroDesc')} />
      <Spin spinning={loading}>
        <Form form={form} layout="vertical">
          <Card size="small" title={t('notifyRules')} style={{ marginBottom: 16 }}>
            <Row gutter={24}>
              <Col xs={24} lg={12}>
                <SwitchRow name="enabled" label={t('notifyEnabled')} />
                <SwitchRow name="notify_offline" label={t('notifyOffline')} hint={t('notifyOfflineHint')} />
                <SwitchRow name="notify_recovery" label={t('notifyRecovery')} hint={t('notifyRecoveryHint')} />
              </Col>
              <Col xs={24} lg={12}>
                <Form.Item name="offline_alert_secs" label={t('offlineAlertSecs')} extra={t('offlineAlertSecsHint').replace('{min}', String(MIN_OFFLINE_ALERT_SECS))} rules={[{ validator: (_, v) => Number(v) >= MIN_OFFLINE_ALERT_SECS ? Promise.resolve() : Promise.reject(new Error(t('offlineAlertSecsTooSmall').replace('{min}', String(MIN_OFFLINE_ALERT_SECS)))) }]}>
                  <InputNumber min={MIN_OFFLINE_ALERT_SECS} style={{ width: 180 }} addonAfter={t('seconds')} />
                </Form.Item>
                <Form.Item name="repeat_alert_minutes" label={t('repeatAlertMinutes')} extra={t('repeatAlertMinutesHint')}>
                  <InputNumber min={0} style={{ width: 180 }} addonAfter={t('minutes')} />
                </Form.Item>
                <SwitchRow name="notify_version_outdated" label={t('notifyVersionOutdated')} hint={t('notifyVersionOutdatedHint')} />
              </Col>
            </Row>
            {globallyEnabled && !telegramEnabled && !emailEnabled && <Alert type="warning" showIcon message={t('notifyNoChannelsHint')} style={{ marginTop: 8 }} />}
          </Card>

          <Card size="small" title={t('notificationChannels')} style={{ marginBottom: 16 }}>
            <Row gutter={16}>
              <Col xs={24} lg={12}>
                <Card size="small" title={<Space>Telegram <ChannelStatus enabled={telegramEnabled} configured={telegramConfigured} t={t} /></Space>} style={{ height: '100%' }}>
                  <SwitchRow name="telegram_enabled" label={t('channelEnableLabel')} />
                  <Form.Item name="telegram_bot_token" label="Bot Token" extra={t('credentialKeepHint')}>
                    <Input.Password autoComplete="off" placeholder={cfg?.telegram_bot_token_set ? t('credentialConfigured') : t('credentialEmpty')} />
                  </Form.Item>
                  <Form.Item name="telegram_chat_id" label="Chat ID" extra={t('telegramChatIdHint')}>
                    <Input autoComplete="off" placeholder="-1001234567890" />
                  </Form.Item>
                  <Button icon={<SendOutlined />} loading={testing === 'telegram'} onClick={() => void onTest('telegram')}>{t('saveAndTest')}</Button>
                </Card>
              </Col>
              <Col xs={24} lg={12}>
                <Card size="small" title={<Space>{t('email')} <ChannelStatus enabled={emailEnabled} configured={emailConfigured} t={t} /></Space>} style={{ height: '100%' }}>
                  <SwitchRow name="email_enabled" label={t('channelEnableLabel')} />
                  <Row gutter={16}>
                    <Col xs={24} md={12}><Form.Item name="smtp_host" label={t('smtpHost')}><Input autoComplete="off" placeholder="smtp.example.com" /></Form.Item></Col>
                    <Col xs={12} md={6}><Form.Item name="smtp_port" label={t('smtpPort')} extra={t('smtpPortHint')}><InputNumber min={1} max={65535} style={{ width: '100%' }} placeholder="465" /></Form.Item></Col>
                    <Col xs={12} md={6}><Form.Item name="smtp_tls" label={t('smtpTls')} extra={t('smtpTlsHint')} valuePropName="checked"><Switch aria-label={t('smtpTls')} /></Form.Item></Col>
                    <Col xs={24} md={12}><Form.Item name="smtp_username" label={t('smtpUsername')}><Input autoComplete="off" placeholder="ops@example.com" /></Form.Item></Col>
                    <Col xs={24} md={12}><Form.Item name="smtp_password" label={t('smtpPassword')} extra={t('credentialKeepHint')}><Input.Password autoComplete="new-password" placeholder={cfg?.smtp_password_set ? t('credentialConfigured') : t('credentialEmpty')} /></Form.Item></Col>
                    <Col xs={24} md={12}><Form.Item name="smtp_from" label={t('smtpFrom')} extra={t('smtpFromHint')}><Input autoComplete="off" placeholder="ops@example.com" /></Form.Item></Col>
                    <Col xs={24}><Form.Item name="smtp_to" label={t('smtpTo')} extra={t('smtpToHint')}><Input autoComplete="off" placeholder="admin@example.com" /></Form.Item></Col>
                  </Row>
                  <Button icon={<SendOutlined />} loading={testing === 'email'} onClick={() => void onTest('email')}>{t('saveAndTest')}</Button>
                </Card>
              </Col>
            </Row>
          </Card>

          <Card size="small" title={t('deliveryHistory')}>
            <Spin spinning={historyLoading}>
              {history.length === 0 ? <Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('notifyHistoryEmpty')} /> : <Table<NotifyHistoryEntry> dataSource={history} columns={columns} rowKey="id" pagination={false} scroll={{ x: 860 }} size="small" />}
            </Spin>
          </Card>

          <Text type="secondary" style={{ display: 'block', fontSize: 12, marginTop: 12 }}>{t('notifyCredentialNote')}</Text>
        </Form>
      </Spin>
    </Card>
  );
}
