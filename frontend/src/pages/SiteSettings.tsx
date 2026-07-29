import { Card, Form, Input, Button, message, Spin, Result, Typography, Select, Alert } from 'antd';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, SiteConfig } from '../api/types';
import { useI18n } from '../i18n/context';
import { invalidateSite } from '../hooks/useSite';
import { invalidateSiteNotice } from '../hooks/useSiteNotice';
import { renderMarkdown } from '../utils/markdown';

const { Text } = Typography;

// Mirrors the caps enforced in service::site. Duplicated deliberately: the
// server is the authority (it truncates), and these only exist so the user is
// told before submitting rather than silently losing the tail.
const MAX_NAME = 64;
const MAX_SUBTITLE = 128;
const MAX_ANNOUNCEMENT = 4000;
const MAX_CONTACT = 256;

// The severities the backend accepts. Kept as a const tuple so the preview can
// hand antd's Alert a properly narrowed type — Form.useWatch only knows the
// field is a string.
const ALERT_TYPES = ['info', 'success', 'warning', 'error'] as const;
type AlertType = (typeof ALERT_TYPES)[number];

/**
 * v1.3.0: site identity — name, subtitle, announcement, support contact.
 *
 * Separate from "System Settings", which is registration policy (open
 * registration / allowed plans / default plan). Different concerns, and folding
 * them into one page would make both harder to scan.
 */
export default function SiteSettings() {
  const { t } = useI18n();
  const [form] = Form.useForm<SiteConfig>();
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [failed, setFailed] = useState(false);
  const [initial, setInitial] = useState<SiteConfig | null>(null);
  // Watched so the preview below the textarea updates as it is typed. The
  // markdown subset is small but not obvious, and a preview is cheaper than
  // making the operator save and navigate to see what they wrote.
  const announcement = Form.useWatch('announcement', form);
  const announcementType = Form.useWatch('announcement_type', form);
  const previewType: AlertType = (ALERT_TYPES as readonly string[]).includes(announcementType)
    ? (announcementType as AlertType)
    : 'info';

  const load = useCallback(async () => {
    setLoading(true);
    setFailed(false);
    try {
      const res = await api.get<unknown, ApiEnvelope<SiteConfig>>('/admin/settings/site');
      if (res.code !== 0 || !res.data) {
        setFailed(true);
        return;
      }
      // Held in state and handed to the Form as initialValues rather than
      // pushed with setFieldsValue: the Form is not mounted yet during the
      // loading render, and calling into a disconnected form instance is what
      // antd's "useForm is not connected to any Form element" warning is about.
      setInitial(res.data);
    } catch {
      setFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const onSave = async () => {
    const values = await form.validateFields();
    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<SiteConfig>>('/admin/settings/site', values);
      if (res.code !== 0) {
        message.error(res.message || t('settingsSaveFailed'));
        return;
      }
      // The server trims and clamps; show what actually landed rather than
      // leaving the form displaying input that was silently adjusted.
      if (res.data) form.setFieldsValue(res.data);
      // The brand is cached module-wide and rendered by the sidebar and login
      // page — drop it so the new name appears without a hard refresh.
      invalidateSite();
      invalidateSiteNotice();
      message.success(t('settingsSaved'));
    } catch {
      message.error(t('settingsSaveFailed'));
    } finally {
      setSaving(false);
    }
  };

  if (loading) return <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;

  if (failed) {
    return (
      <Result
        status="warning"
        title={t('settingsLoadFailed')}
        extra={<Button type="primary" onClick={load}>{t('refresh')}</Button>}
      />
    );
  }

  return (
    <Card
      title={t('siteSettings')}
      extra={<Button type="primary" loading={saving} onClick={onSave}>{t('save')}</Button>}
    >
      <Form form={form} layout="vertical" style={{ maxWidth: 640 }} initialValues={initial ?? undefined}>
        <Form.Item
          name="site_name"
          label={t('siteName')}
          extra={t('siteNameHint')}
          rules={[{ max: MAX_NAME, message: t('siteFieldTooLong') }]}
        >
          <Input placeholder="RelayPanel" showCount maxLength={MAX_NAME} />
        </Form.Item>
        <Form.Item
          name="subtitle"
          label={t('siteSubtitle')}
          extra={t('siteSubtitleHint')}
          rules={[{ max: MAX_SUBTITLE, message: t('siteFieldTooLong') }]}
        >
          <Input showCount maxLength={MAX_SUBTITLE} />
        </Form.Item>
        <Form.Item
          name="announcement_type"
          label={t('siteAnnouncementType')}
          extra={t('siteAnnouncementTypeHint')}
        >
          <Select
            style={{ maxWidth: 220 }}
            options={[
              { value: 'info', label: t('announcementTypeInfo') },
              { value: 'success', label: t('announcementTypeSuccess') },
              { value: 'warning', label: t('announcementTypeWarning') },
              { value: 'error', label: t('announcementTypeError') },
            ]}
          />
        </Form.Item>
        <Form.Item
          name="announcement"
          label={t('siteAnnouncement')}
          rules={[{ max: MAX_ANNOUNCEMENT, message: t('siteFieldTooLong') }]}
          // No `extra` here on purpose. For a TextArea, antd renders showCount's
          // counter absolutely at bottom:-22px, which is the exact band
          // Form.Item puts extra text in — a hint long enough to reach the
          // right edge runs underneath "118 / 4000".
          //
          // The help sits below the item instead, and the bottom margin has to
          // clear that 22px overhang or the counter simply overlaps the help
          // text rather than the extra text. 28px leaves a visible gap.
          style={{ marginBottom: 28 }}
        >
          <Input.TextArea rows={6} showCount maxLength={MAX_ANNOUNCEMENT} />
        </Form.Item>
        <div style={{ marginBottom: 24 }}>
          <Text type="secondary" style={{ fontSize: 12, display: 'block' }}>
            {t('siteAnnouncementHint')}
          </Text>
          <Text type="secondary" style={{ fontSize: 12, display: 'block', marginTop: 2 }}>
            {t('siteAnnouncementSyntax')}
          </Text>
        </div>
        {/* Preview exactly as users will see it — same Alert, same renderer. */}
        {announcement?.trim() ? (
          <Form.Item label={t('siteAnnouncementPreview')}>
            <Alert
              type={previewType}
              showIcon
              description={
                <div style={{ whiteSpace: 'pre-wrap' }}>{renderMarkdown(announcement)}</div>
              }
            />
          </Form.Item>
        ) : null}
        <Form.Item
          name="contact"
          label={t('siteContact')}
          extra={t('siteContactHint')}
          rules={[{ max: MAX_CONTACT, message: t('siteFieldTooLong') }]}
        >
          <Input showCount maxLength={MAX_CONTACT} />
        </Form.Item>
        <Text type="secondary" style={{ fontSize: 12 }}>{t('siteSettingsHint')}</Text>
      </Form>
    </Card>
  );
}
