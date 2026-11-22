import { useEffect, useState } from 'react';
import { Card, Row, Col, Spin, Button, Space, Modal, Form, Input, message, Typography, Result, Progress, Alert, Tag } from 'antd';
import { LockOutlined, LogoutOutlined, WalletOutlined, ShoppingOutlined, ThunderboltOutlined, ApiOutlined, CustomerServiceOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import api from '../api/client';
import type { ApiEnvelope, UserSelf } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';
import { makePasswordValidator } from '../utils/password';
import { useAuth } from '../auth/useAuth';
import TrafficChart from '../components/TrafficChart';
import SiteAnnouncement from '../components/SiteAnnouncement';
import { useSiteNotice } from '../hooks/useSiteNotice';
import RedeemHistory from '../components/RedeemHistory';

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
  const { logout: authLogout, refreshCurrentUser } = useAuth();
  const notice = useSiteNotice();
  const [me, setMe] = useState<UserSelf | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);

  // Change-password modal state (mirrors MainLayout's modal).
  const [changePwOpen, setChangePwOpen] = useState(false);
  const [pwForm] = Form.useForm();
  const [pwSubmitting, setPwSubmitting] = useState(false);

  // v1.2.0: redeem-code top-up.
  const [redeemOpen, setRedeemOpen] = useState(false);
  const [redeemForm] = Form.useForm();
  const [redeeming, setRedeeming] = useState(false);

  const handleRedeem = async (values: { code: string }) => {
    setRedeeming(true);
    try {
      const res = await api.post<unknown, ApiEnvelope<{ amount: string; balance: string }>>(
        '/user/redeem',
        { code: values.code },
      );
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(
        t('redeemSuccess')
          .replace('{amount}', res.data?.amount ?? '')
          .replace('{balance}', res.data?.balance ?? ''),
      );
      setRedeemOpen(false);
      redeemForm.resetFields();
      load(); // refresh the balance shown above
      void refreshCurrentUser();
    } catch {
      message.error(t('redeemFailed'));
    } finally {
      setRedeeming(false);
    }
  };

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
      authLogout();
      navigate('/login', { replace: true });
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

  const trafficPct = me.traffic_limit > 0
    ? Math.min(100, Math.round((me.traffic_used / me.traffic_limit) * 100))
    : 0;
  const trafficExhausted = me.traffic_limit > 0 && me.traffic_used >= me.traffic_limit;
  const rulePct = me.max_rules > 0
    ? Math.min(100, Math.round((me.current_rules / me.max_rules) * 100))
    : 0;
  const avatarInitial = (me.username.trim()[0] || 'U').toUpperCase();

  return (
    <>
      <SiteAnnouncement />
      {me.suspended && (
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('accountSuspended')}
          description={t('accountSuspendedHint')}
        />
      )}

      {/* Profile Header Card */}
      <Card style={{ marginBottom: 16 }} styles={{ body: { padding: '20px 24px' } }}>
        <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', justifyContent: 'space-between', gap: 16 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 16, minWidth: 0 }}>
            <div style={{
              width: 48, height: 48, borderRadius: 14, flexShrink: 0,
              background: 'linear-gradient(135deg, #6366f1 0%, #4f46e5 100%)',
              boxShadow: '0 4px 12px rgba(79, 70, 229, 0.22)',
              color: '#fff', fontSize: 20, fontWeight: 600,
              display: 'flex', alignItems: 'center', justifyContent: 'center',
            }}>
              {avatarInitial}
            </div>
            <div style={{ minWidth: 0 }}>
              <Space size={8} wrap>
                <Text strong style={{ fontSize: 18 }}>{me.username}</Text>
                <Tag color={me.admin ? 'gold' : 'blue'}>{me.admin ? t('admin') : t('user')}</Tag>
              </Space>
              <div style={{ marginTop: 4, fontSize: 12, color: 'var(--rp-text-tertiary)' }}>
                {t('accountMemberSince')}: <span className="rp-mono">{me.registered_at || '-'}</span>
                {notice.contact && (
                  <span style={{ marginLeft: 14 }}>
                    <CustomerServiceOutlined style={{ marginRight: 4 }} />
                    {t('siteContact')}: <span style={{ color: 'var(--rp-text-secondary)' }}>{notice.contact}</span>
                  </span>
                )}
              </div>
            </div>
          </div>
          <Space wrap>
            <Button icon={<LockOutlined />} onClick={() => setChangePwOpen(true)}>{t('changePassword')}</Button>
            <Button danger icon={<LogoutOutlined />} onClick={logout}>{t('logout')}</Button>
          </Space>
        </div>
      </Card>

      {/* Bento Overview Grid */}
      <Row gutter={[16, 16]} style={{ marginBottom: 16 }}>
        <Col xs={24} sm={12} lg={6}>
          <Card className="rp-kpi-card" styles={{ body: { padding: 20, display: 'flex', flexDirection: 'column', height: '100%' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 12 }}>
              <Text type="secondary" style={{ fontSize: 13 }}>{t('accountBalance')}</Text>
              <span className="rp-icon-tile" style={{ background: '#eef2ff', color: '#4f46e5' }}>
                <WalletOutlined />
              </span>
            </div>
            <div style={{ display: 'flex', alignItems: 'baseline', gap: 4, marginBottom: 14 }}>
              <span style={{ fontSize: 15, fontWeight: 600, color: 'var(--rp-text-secondary)' }}>¥</span>
              <span className="rp-mono" style={{ fontSize: 28, fontWeight: 600, color: 'var(--rp-text)', lineHeight: 1.2 }}>
                {me.balance}
              </span>
            </div>
            <div style={{ marginTop: 'auto' }}>
              <Button
                size="small"
                type="primary"
                icon={<WalletOutlined />}
                onClick={() => { redeemForm.resetFields(); setRedeemOpen(true); }}
              >
                {t('redeem')}
              </Button>
            </div>
          </Card>
        </Col>

        <Col xs={24} sm={12} lg={6}>
          <Card className="rp-kpi-card" styles={{ body: { padding: 20, display: 'flex', flexDirection: 'column', height: '100%' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 12 }}>
              <Text type="secondary" style={{ fontSize: 13 }}>{t('accountPlan')}</Text>
              <span className="rp-icon-tile" style={{ background: '#f5f3ff', color: '#7c3aed' }}>
                <ShoppingOutlined />
              </span>
            </div>
            <div style={{ fontSize: 20, fontWeight: 600, color: 'var(--rp-text)', marginBottom: 6 }}>
              {me.plan_name || '-'}
            </div>
            <div style={{ fontSize: 12, color: 'var(--rp-text-tertiary)', marginBottom: 12 }}>
              {t('accountPlanExpiry')}:{' '}
              {me.plan_expire_at ? <span className="rp-mono">{me.plan_expire_at}</span> : t('unlimited')}
            </div>
            {!me.admin && (
              <div style={{ marginTop: 'auto' }}>
                <Button size="small" onClick={() => navigate('/shop')}>{t('shop')}</Button>
              </div>
            )}
          </Card>
        </Col>

        <Col xs={24} sm={12} lg={6}>
          <Card className="rp-kpi-card" styles={{ body: { padding: 20, display: 'flex', flexDirection: 'column', height: '100%' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 12 }}>
              <Text type="secondary" style={{ fontSize: 13 }}>{t('accountTrafficUsed')}</Text>
              <span className="rp-icon-tile" style={{ background: '#ecfdf5', color: '#059669' }}>
                <ThunderboltOutlined />
              </span>
            </div>
            <div className="rp-mono" style={{ fontSize: 20, fontWeight: 600, color: 'var(--rp-text)', marginBottom: 4 }}>
              {formatBytes(me.traffic_used)}
            </div>
            <div style={{ fontSize: 12, color: 'var(--rp-text-tertiary)', marginBottom: 10 }}>
              / {me.traffic_limit > 0 ? formatBytes(me.traffic_limit) : t('unlimited')}
            </div>
            <div style={{ marginTop: 'auto' }}>
              {me.traffic_limit > 0 ? (
                <Progress
                  percent={trafficPct}
                  size="small"
                  strokeColor={trafficExhausted ? '#e11d48' : '#4f46e5'}
                  status={trafficExhausted ? 'exception' : 'normal'}
                  style={{ marginBottom: 0 }}
                />
              ) : (
                <Tag color="green">{t('unlimited')}</Tag>
              )}
            </div>
          </Card>
        </Col>

        <Col xs={24} sm={12} lg={6}>
          <Card className="rp-kpi-card" styles={{ body: { padding: 20, display: 'flex', flexDirection: 'column', height: '100%' } }}>
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', marginBottom: 12 }}>
              <Text type="secondary" style={{ fontSize: 13 }}>{t('accountRulesLimit')}</Text>
              <span className="rp-icon-tile" style={{ background: '#f0fdfa', color: '#0d9488' }}>
                <ApiOutlined />
              </span>
            </div>
            <div className="rp-mono" style={{ fontSize: 22, fontWeight: 600, color: 'var(--rp-text)', marginBottom: 8 }}>
              {me.current_rules}{' '}
              <span style={{ fontSize: 14, fontWeight: 400, color: 'var(--rp-text-tertiary)' }}>
                / {me.max_rules > 0 ? me.max_rules : t('unlimited')}
              </span>
            </div>
            {me.max_rules > 0 && (
              <Progress percent={rulePct} size="small" strokeColor="#0d9488" showInfo={false} style={{ marginBottom: 10 }} />
            )}
            <div style={{ marginTop: 'auto' }}>
              <div style={{ fontSize: 12, color: 'var(--rp-text-tertiary)', marginBottom: 4 }}>
                {t('accountAvailableGroups')}
              </div>
              {me.all_groups ? (
                <Tag color="green">{t('allGroups')}</Tag>
              ) : me.available_groups && me.available_groups.length > 0 ? (
                <Space size={[4, 4]} wrap>
                  {me.available_groups.map((name) => <Tag key={name}>{name}</Tag>)}
                </Space>
              ) : (
                <Text type="secondary" style={{ fontSize: 12 }}>{t('noneAssigned')}</Text>
              )}
            </div>
          </Card>
        </Col>
      </Row>

      {/* v1.2.0: the user's own traffic trend. No rule filter here — the API
          pins non-admins to their own uid, so the total IS their usage.

          Hidden for admins: they already get the fleet-wide chart (with a
          per-rule drill-down) on the dashboard, and an admin's "personal"
          traffic is a near-meaningless number. The same card on two pages is
          just noise. */}
      {/* v1.2.4: the user's own top-up history. Above the traffic chart —
          "where did my balance come from" is the question that brings people to
          this page, and it should not sit below a chart. Purchase history is
          not duplicated here; it stays on the shop page. */}
      <RedeemHistory />

      {!me.admin && <TrafficChart />}

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
            rules={[
              { required: true, message: t('passwordRequired') },
              { validator: makePasswordValidator(t('newPasswordTooShort'), t('passwordTooLong')) },
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
        </Form>
      </Modal>

      {/* v1.2.0: redeem a top-up code. The backend normalizes input (case,
          dashes, whitespace, O/0 and I/L/1 confusions), so the field is
          deliberately permissive — no client-side format policing that would
          reject a code the server would have accepted. */}
      <Modal
        title={t('redeem')}
        open={redeemOpen}
        onCancel={() => setRedeemOpen(false)}
        onOk={() => redeemForm.submit()}
        okText={t('redeemConfirm')}
        cancelText={t('cancel')}
        confirmLoading={redeeming}
      >
        <Form form={redeemForm} onFinish={handleRedeem} layout="vertical">
          <Form.Item
            name="code"
            label={t('redeemCode')}
            extra={t('redeemCodeHint')}
            rules={[{ required: true, message: t('redeemCodeRequired') }]}
          >
            <Input placeholder="XXXX-XXXX-XXXX-XXXX" autoComplete="off" />
          </Form.Item>
        </Form>
      </Modal>
    </>
  );
}
