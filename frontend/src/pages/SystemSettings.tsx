import { Card, Form, Switch, Select, Button, message, Spin, Result, Typography } from 'antd';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Plan, RegistrationSettings } from '../api/types';
import { useI18n } from '../i18n/context';
import NotifySettings from './NotifySettings';

const { Text } = Typography;

/** v0.4.10 PR3 / v0.4.21 PR2: admin system settings page.
 *  Manages registration toggle, allowed registration plans (multi-select),
 *  and the default selected plan. */
export default function SystemSettings() {
  const { t } = useI18n();
  const [form] = Form.useForm();
  const [plans, setPlans] = useState<Plan[]>([]);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [saving, setSaving] = useState(false);

  const load = async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const [settingsRes, plansRes] = await Promise.all([
        api.get<unknown, ApiEnvelope<RegistrationSettings>>('/admin/settings/registration'),
        api.get<unknown, ApiEnvelope<Plan[]>>('/admin/plans'),
      ]);
      if (settingsRes.data) {
        form.setFieldsValue({
          registration_enabled: settingsRes.data.registration_enabled,
          default_registration_plan_id: settingsRes.data.default_registration_plan_id,
          allowed_plan_ids: settingsRes.data.allowed_plan_ids,
        });
      }
      setPlans(plansRes.data || []);
    } catch {
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const onSave = async () => {
    const values = await form.validateFields();

    // Client-side guard: allowed_plan_ids must not be empty.
    const allowed = (values.allowed_plan_ids as number[]) || [];
    if (allowed.length === 0) {
      message.error(t('allowedPlansRequired'));
      return;
    }

    // Client-side guard: default_plan_id must be in allowed_plan_ids.
    const defaultId = values.default_registration_plan_id as number;
    if (!allowed.includes(defaultId)) {
      message.error(t('defaultPlanNotAllowed'));
      return;
    }

    setSaving(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<RegistrationSettings>>(
        '/admin/settings/registration',
        {
          enabled: values.registration_enabled,
          default_plan_id: defaultId,
          allowed_plan_ids: allowed,
        }
      );
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      message.success(t('settingsSaved'));
    } catch {
      message.error(t('settingsSaveFailed'));
    } finally {
      setSaving(false);
    }
  };

  // When allowed_plan_ids changes, clear default if it's no longer valid.
  const handleAllowedChange = (newAllowed: number[]) => {
    const currentDefault: number = form.getFieldValue('default_registration_plan_id');
    if (newAllowed.length > 0 && !newAllowed.includes(currentDefault)) {
      form.setFieldValue('default_registration_plan_id', newAllowed[0]);
    }
  };

  if (loading) {
    return <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;
  }
  if (loadFailed) {
    return (
      <Result
        status="warning"
        title={t('settingsLoadFailed')}
        extra={<Button type="primary" onClick={load}>{t('refresh')}</Button>}
      />
    );
  }

  const planOptions = plans.map((p) => ({ value: p.id, label: `${p.name} (${p.max_rules} ${t('rules')})` }));

  return (
    <>
    <Card
      title={t('systemSettings')}
      extra={<Button type="primary" loading={saving} onClick={onSave}>{t('save')}</Button>}
    >
      <Form form={form} layout="vertical">
        <Form.Item
          name="registration_enabled"
          label={t('registrationEnabled')}
          valuePropName="checked"
        >
          <Switch />
        </Form.Item>

        <Form.Item
          name="allowed_plan_ids"
          label={t('allowedPlans')}
          extra={t('allowedPlansHint')}
          rules={[{ required: true, message: t('allowedPlansRequired') }]}
        >
          <Select
            mode="multiple"
            options={planOptions}
            onChange={handleAllowedChange}
            placeholder={t('allowedPlans')}
          />
        </Form.Item>

        <Form.Item noStyle shouldUpdate={(prev, cur) => prev.allowed_plan_ids !== cur.allowed_plan_ids}>
          {({ getFieldValue }) => {
            const allowedIds: number[] = getFieldValue('allowed_plan_ids') || [];
            const defaultOptions = planOptions.filter(o => allowedIds.includes(o.value));
            return (
              <Form.Item
                name="default_registration_plan_id"
                label={t('defaultPlan')}
                rules={[{ required: true, message: t('defaultPlanRequired') }]}
                extra={allowedIds.length === 0 ? t('allowedPlansRequired') : undefined}
              >
                <Select
                  options={defaultOptions}
                  placeholder={t('selectPlan')}
                  disabled={allowedIds.length === 0}
                />
              </Form.Item>
            );
          }}
        </Form.Item>

        <Text type="secondary" style={{ fontSize: 12 }}>
          {t('registrationSettingsHint')}
        </Text>
      </Form>
    </Card>
    {/* v1.2.0: node-offline alerting. Its own card + form so saving one
        section can never submit the other. */}
    <NotifySettings />
    </>
  );
}
