import { Card, Row, Col, Button, Spin, Tag, Modal, Table, Typography, message, Result, Alert, Space } from 'antd';
import { ShoppingOutlined, ReloadOutlined } from '@ant-design/icons';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Plan, Order, UserSelf } from '../api/types';
import { useI18n } from '../i18n/context';
import { formatBytes } from '../utils/format';

const { Text, Title } = Typography;

/**
 * v1.0.8: self-service shop. Lists purchasable (non-hidden) plans as cards,
 * with a confirm modal before buying. Buying charges balance, stacks traffic
 * onto the current quota (per the "流量叠加到当前额度" note), and records an
 * order. The order-history table shows past purchases (snapshotted plan_name +
 * price). A suspended user can still buy (buying does NOT auto-unsuspend).
 */
export default function Shop() {
  const { t } = useI18n();
  const [plans, setPlans] = useState<Plan[]>([]);
  const [orders, setOrders] = useState<Order[]>([]);
  const [me, setMe] = useState<UserSelf | null>(null);
  const [loading, setLoading] = useState(true);
  const [loadFailed, setLoadFailed] = useState(false);
  const [buying, setBuying] = useState<Plan | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setLoadFailed(false);
    try {
      const [plansRes, ordersRes, meRes] = await Promise.all([
        api.get<unknown, ApiEnvelope<Plan[]>>('/plans'),
        api.get<unknown, ApiEnvelope<Order[]>>('/user/orders'),
        api.get<unknown, ApiEnvelope<UserSelf>>('/user/me'),
      ]);
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
      if (res.code !== 0) { message.error(res.message); return; }
      message.success(t('purchaseSuccess'));
      setBuying(null);
      load();
    } catch {
      message.error(t('purchaseFailed'));
    } finally {
      setSubmitting(false);
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

      {/* v1.0.8: suspended banner — buying is still allowed (does not auto-clear). */}
      {me?.suspended && (
        <Alert
          type="warning"
          showIcon
          style={{ marginBottom: 16 }}
          message={t('accountSuspended')}
          description={t('shopSuspendedHint')}
        />
      )}

      {/* Balance + the "流量叠加" note. */}
      {me && (
        <Card size="small" style={{ marginBottom: 16 }}>
          <Space>
            <Text strong>{t('accountBalance')}:</Text>
            <span className="rp-mono">{me.balance}</span>
            <Text type="secondary" style={{ marginLeft: 16 }}>·</Text>
            <Text type="secondary">{t('shopTrafficStacksHint')}</Text>
          </Space>
        </Card>
      )}

      <Row gutter={[16, 16]}>
        {plans.length === 0 && (
          <Col span={24}>
            <Card><Text type="secondary">{t('noPlansAvailable')}</Text></Card>
          </Col>
        )}
        {plans.map((p) => (
          <Col xs={24} sm={12} md={8} key={p.id}>
            <Card
              title={<Space><Text strong>{p.name}</Text>{p.plan_type === 'time' && <Tag color="purple">{t('planTypeTime')}</Tag>}</Space>}
              extra={p.description ? <Text type="secondary" style={{ fontSize: 12 }}>{p.description}</Text> : null}
            >
              <div style={{ marginBottom: 8 }}>
                <Title level={3} style={{ margin: 0 }}><span className="rp-mono">{p.price}</span></Title>
              </div>
              <div style={{ color: 'var(--rp-text-secondary)', fontSize: 13, lineHeight: 1.8 }}>
                <div>{t('planTraffic')}: {p.traffic > 0 ? formatBytes(p.traffic) : t('unlimited')}</div>
                <div>{t('planMaxRules')}: {p.max_rules}</div>
                {p.duration_days > 0 && <div>{t('planDuration')}: {p.duration_days} {t('days')}</div>}
                {/* v1.0.9: device groups this plan grants on purchase. Names are
                    resolved server-side (device_group_names) — the buyer isn't
                    authorized for these groups yet, so the client can't resolve
                    the ids itself. */}
                {p.grant_all_groups ? (
                  <div>{t('planGrantGroups')}: <Tag color="gold">{t('planGrantAll')}</Tag></div>
                ) : (p.device_group_names && p.device_group_names.length > 0) ? (
                  <div>{t('planGrantGroups')}: {p.device_group_names.join(', ')}</div>
                ) : null}
                {p.reset_traffic && <div><Tag color="green">{t('planResetTraffic')}</Tag></div>}
              </div>
              <Button type="primary" block style={{ marginTop: 16 }} onClick={() => setBuying(p)}>
                {t('buyNow')}
              </Button>
            </Card>
          </Col>
        ))}
      </Row>

      {/* Order history. */}
      <Card title={t('orderHistory')} style={{ marginTop: 24 }}>
        <Table
          dataSource={orders}
          columns={orderColumns}
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
            <Alert type="info" showIcon style={{ marginTop: 8 }} message={t('shopTrafficStacksHint')} />
          </div>
        )}
      </Modal>
    </>
  );
}
