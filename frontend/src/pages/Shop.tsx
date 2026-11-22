import { Card, Row, Col, Button, Spin, Tag, Modal, Table, Typography, message, Result, Alert, Space, Form, Input, Empty } from 'antd';
import { ShoppingOutlined, ReloadOutlined, WalletOutlined, CheckCircleFilled } from '@ant-design/icons';
import { useCallback, useContext, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Plan, Order, UserSelf } from '../api/types';
import { useI18n } from '../i18n/context';
import { AuthContext } from '../auth/AuthContext';
import { useTableScroll } from '../hooks/useIsMobile';
import { formatBytes } from '../utils/format';

  const { Text } = Typography;

/**
 * v1.0.8: self-service shop. Lists purchasable (non-hidden) plans as cards,
 * with a confirm modal before buying. Buying charges balance, stacks traffic
 * onto the current quota (per the "流量叠加到当前额度" note), and records an
 * order. The order-history table shows past purchases (snapshotted plan_name +
 * price). A suspended user can still buy (buying does NOT auto-unsuspend).
 */
export default function Shop() {
  const tableScroll = useTableScroll();
  const { t } = useI18n();
  const auth = useContext(AuthContext);
  const [plans, setPlans] = useState<Plan[]>([]);
  const [orders, setOrders] = useState<Order[]>([]);
  const [me, setMe] = useState<UserSelf | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [buying, setBuying] = useState<Plan | null>(null);
  const [submitting, setSubmitting] = useState(false);
  // v1.2.0: redeem-code top-up, mirrored from the account page so a user who
  // finds their balance short can fix it without leaving the shop.
  const [redeemOpen, setRedeemOpen] = useState(false);
  const [redeemForm] = Form.useForm();
  const [redeeming, setRedeeming] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const [plansRes, ordersRes, meRes] = await Promise.all([
        api.get<unknown, ApiEnvelope<Plan[]>>('/plans'),
        api.get<unknown, ApiEnvelope<Order[]>>('/user/orders'),
        api.get<unknown, ApiEnvelope<UserSelf>>('/user/me'),
      ]);
      if (plansRes.code !== 0 || ordersRes.code !== 0 || meRes.code !== 0) {
        setLoadFailed(true);
        return;
      }
      setPlans(plansRes.data || []);
      setOrders(ordersRes.data || []);
      setMe(meRes.data || null);
    } catch {
      setLoadFailed(true);
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { load(); }, [load]);

  const handleBuy = async () => {
    if (!buying) return;
    setSubmitting(true);
    try {
      const res = await api.post<unknown, ApiEnvelope<null>>('/user/buy-plan', { plan_id: buying.id });
      if (res.code !== 0) {
        message.error(res.message);
        // A rejected one-time claim means our list is stale; refresh it so the
        // card flips to "claimed".
        if (res.code === 409) { setBuying(null); load(); }
        return;
      }
      message.success(t('purchaseSuccess'));
      setBuying(null);
      load();
      void auth?.refreshCurrentUser();
    } catch {
      message.error(t('purchaseFailed'));
    } finally {
      setSubmitting(false);
    }
  };

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
      load(); // the balance shown above the plan cards must reflect the top-up
      void auth?.refreshCurrentUser();
    } catch {
      message.error(t('redeemFailed'));
    } finally {
      setRedeeming(false);
    }
  };

  if (loading) {
    return <div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>;
  }

  if (loadFailed) {
    return (
      <Result
        status="warning"
        title={t('shopLoadFailed')}
        extra={<Button type="primary" onClick={load}>{t('refresh')}</Button>}
      />
    );
  }

  const orderColumns = [
    { title: t('orderId'), dataIndex: 'id', key: 'id', width: 70 },
    { title: t('planName'), dataIndex: 'plan_name', key: 'plan_name' },
    { title: t('planPrice'), dataIndex: 'price', key: 'price', render: (v: string) => <span className="rp-mono">{v}</span> },
    { title: t('purchaseTime'), dataIndex: 'created_at', key: 'created_at', render: (v: string) => <span className="rp-mono">{v}</span> },
  ];

  return (
    <>
      <div className="rp-page-header">
        <h2 className="rp-page-title"><ShoppingOutlined /> {t('shop')}</h2>
        <Button icon={<ReloadOutlined />} onClick={load}>{t('refresh')}</Button>
      </div>

      {me?.admin && (
        <Alert
          type="info"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('adminPlanNotNeeded')}
        />
      )}

      {/* v1.0.8: suspended banner — buying is still allowed (does not auto-clear). */}
      {me?.suspended && (
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16 }}
          title={t('accountSuspended')}
          description={t('shopSuspendedHint')}
        />
      )}

      {/* Balance + top-up + the "流量叠加" note.

          v1.2.0: the redeem entry point lives HERE as well as on the account
          page, because this is where a user actually discovers they can't
          afford a plan. Making them go hunt for it on another page is the
          difference between a sale and an abandoned one. */}
      {me && (
        <Card style={{ marginBottom: 20 }} styles={{ body: { padding: '16px 20px' } }}>
          <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', justifyContent: 'space-between', gap: 12 }}>
            <Space size={12} wrap>
              <span className="rp-icon-tile" style={{ width: 36, height: 36, fontSize: 16, background: '#eef2ff', color: '#4f46e5' }}>
                <WalletOutlined />
              </span>
              <div>
                <div style={{ fontSize: 12, color: 'var(--rp-text-tertiary)' }}>{t('accountBalance')}</div>
                <div style={{ display: 'flex', alignItems: 'baseline', gap: 3 }}>
                  <span style={{ fontSize: 13, fontWeight: 600, color: 'var(--rp-text-secondary)' }}>¥</span>
                  <span className="rp-mono" style={{ fontSize: 20, fontWeight: 600, color: 'var(--rp-text)' }}>{me.balance}</span>
                </div>
              </div>
              <Button
                type="primary"
                icon={<WalletOutlined />}
                onClick={() => { redeemForm.resetFields(); setRedeemOpen(true); }}
              >
                {t('redeem')}
              </Button>
            </Space>
            <Text type="secondary" style={{ fontSize: 12 }}>{t('shopTrafficStacksHint')}</Text>
          </div>
        </Card>
      )}

      <Row gutter={[16, 16]}>
        {plans.length === 0 && (
          <Col span={24}>
            <Card><Empty image={Empty.PRESENTED_IMAGE_SIMPLE} description={t('noPlansAvailable')} /></Card>
          </Col>
        )}
        {plans.map((p) => {
          const isCurrent = me?.plan_id != null && me.plan_id === p.id;
          const isFeatured = isCurrent || p.grant_all_groups;
          return (
            <Col xs={24} sm={12} lg={8} key={p.id} style={{ display: 'flex' }}>
              <Card className={`rp-plan-card${isFeatured ? ' is-featured' : ''}`}>
                <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'flex-start', gap: 8, marginBottom: 6 }}>
                  <Text strong style={{ fontSize: 16 }}>{p.name}</Text>
                  <Space size={4}>
                    {p.plan_type === 'time' ? <Tag color="purple">{t('planTypeTime')}</Tag> : <Tag color="blue">{t('planTypeData')}</Tag>}
                  </Space>
                </div>
                {p.description ? (
                  <div style={{ fontSize: 12, color: 'var(--rp-text-tertiary)', marginBottom: 14, minHeight: 18 }}>
                    {p.description}
                  </div>
                ) : (
                  <div style={{ marginBottom: 14, minHeight: 18 }} />
                )}

                <div style={{
                  display: 'flex', alignItems: 'baseline', gap: 4,
                  paddingBottom: 16, marginBottom: 16,
                  borderBottom: '1px solid var(--rp-border)',
                }}>
                  <span style={{ fontSize: 16, fontWeight: 600, color: 'var(--rp-text-secondary)' }}>¥</span>
                  <span className="rp-mono" style={{ fontSize: 32, fontWeight: 700, color: 'var(--rp-text)', lineHeight: 1.1, letterSpacing: -0.5 }}>
                    {p.price}
                  </span>
                  {p.duration_days > 0 && (
                    <span style={{ fontSize: 13, color: 'var(--rp-text-tertiary)' }}>
                      / {p.duration_days} {t('days')}
                    </span>
                  )}
                </div>

                <div style={{ marginBottom: 20 }}>
                  <div className="rp-plan-feature">
                    <CheckCircleFilled style={{ color: '#4f46e5', marginTop: 4, fontSize: 13 }} />
                    <span>{t('planTraffic')}: <Text strong>{p.traffic > 0 ? formatBytes(p.traffic) : t('unlimited')}</Text></span>
                  </div>
                  <div className="rp-plan-feature">
                    <CheckCircleFilled style={{ color: '#4f46e5', marginTop: 4, fontSize: 13 }} />
                    <span>{t('planMaxRules')}: <Text strong>{p.max_rules}</Text></span>
                  </div>
                  {p.duration_days > 0 && (
                    <div className="rp-plan-feature">
                      <CheckCircleFilled style={{ color: '#4f46e5', marginTop: 4, fontSize: 13 }} />
                      <span>{t('planDuration')}: <Text strong>{p.duration_days} {t('days')}</Text></span>
                    </div>
                  )}
                  <div className="rp-plan-feature">
                    <CheckCircleFilled style={{ color: '#4f46e5', marginTop: 4, fontSize: 13 }} />
                    <span>
                      {t('planGrantGroups')}:{' '}
                      {p.grant_all_groups ? (
                        <Tag color="gold" style={{ marginInlineEnd: 0 }}>{t('planGrantAll')}</Tag>
                      ) : (p.device_group_names && p.device_group_names.length > 0) ? (
                        <Text strong>{p.device_group_names.join(', ')}</Text>
                      ) : (
                        <Text type="secondary">{t('planGrantNone')}</Text>
                      )}
                    </span>
                  </div>
                  {p.reset_traffic && (
                    <div className="rp-plan-feature">
                      <CheckCircleFilled style={{ color: '#059669', marginTop: 4, fontSize: 13 }} />
                      <span><Tag color="green" style={{ marginInlineEnd: 0 }}>{t('planResetTraffic')}</Tag></span>
                    </div>
                  )}
                </div>

                <div style={{ marginTop: 'auto' }}>
                  <Button
                    type={p.claimed === true ? 'default' : 'primary'}
                    block
                    size="large"
                    disabled={p.claimed === true || me?.admin === true}
                    onClick={() => setBuying(p)}
                  >
                    {p.claimed === true ? t('planClaimed') : t('buyNow')}
                  </Button>
                </div>
              </Card>
            </Col>
          );
        })}
      </Row>

      {/* Order history. */}
      <Card title={t('orderHistory')} style={{ marginTop: 24 }}>
        <Table
          dataSource={orders}
          columns={orderColumns}
          scroll={tableScroll}
          rowKey="id"
          pagination={{ pageSize: 10 }}
          size="small"
          locale={{ emptyText: t('noOrders') }}
        />
      </Card>

      {/* Purchase confirm. */}
      <Modal
        open={!!buying}
        onCancel={() => setBuying(null)}
        onOk={handleBuy}
        okText={t('confirmPurchase')}
        cancelText={t('cancel')}
        confirmLoading={submitting}
        title={t('purchaseConfirmTitle')}
      >
        {buying && (
          <div>
            <p>{t('planName')}: <Text strong>{buying.name}</Text></p>
            <p>{t('planPrice')}: <span className="rp-mono">{buying.price}</span></p>
            {me && <p>{t('accountBalance')}: <span className="rp-mono">{me.balance}</span></p>}
            {/* v1.0.9: buying a DIFFERENT plan is a switch — the current plan's
                remaining traffic and expiry are wiped. Warn explicitly. Buying
                the SAME plan (or having none) just renews/stacks. */}
            {me?.plan_id != null && buying.id !== me.plan_id ? (
              <Alert type="warning" showIcon style={{ marginTop: 8 }} title={t('shopSwitchPlanWarning')} />
            ) : (
              <Alert type="info" showIcon style={{ marginTop: 8 }} title={t('shopTrafficStacksHint')} />
            )}
          </div>
        )}
      </Modal>

      {/* Same permissive field as the account page: the backend normalizes
          case, dashes, whitespace and O/0 · I/L/1 confusions, so no
          client-side format policing that would reject a valid code. */}
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
