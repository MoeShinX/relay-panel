import { Card, Table, Tabs } from 'antd';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, MyRedeemRecord, Order } from '../api/types';
import { useI18n } from '../i18n/context';

/**
 * v1.3.0: the user's own money history on the account page — redeem-code
 * top-ups and plan purchases.
 *
 * Two tabs in one card rather than two stacked cards: they answer the same
 * question ("where did my balance go"), and stacking them pushed the traffic
 * chart far below the fold on the page most regular users land on.
 *
 * The purchase table is deliberately NOT removed from the shop page — buying
 * and then seeing the receipt in place is worth the duplicated read.
 */
export default function AccountRecords() {
  const { t } = useI18n();
  const [redeems, setRedeems] = useState<MyRedeemRecord[]>([]);
  const [orders, setOrders] = useState<Order[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    Promise.all([
      api.get<unknown, ApiEnvelope<MyRedeemRecord[]>>('/user/redeem-records'),
      api.get<unknown, ApiEnvelope<Order[]>>('/user/orders'),
    ])
      .then(([r, o]) => {
        if (!alive) return;
        setRedeems(r.data ?? []);
        setOrders(o.data ?? []);
      })
      // An empty history and an unreachable one look the same here on purpose:
      // neither is worth an error toast on the landing page.
      .catch(() => {})
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, []);

  const redeemColumns = [
    {
      title: t('redeemCode'),
      dataIndex: 'code',
      key: 'code',
      // Masked server-side to the last group — see MyRedeemRecord.
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('rechargeAmount'),
      dataIndex: 'amount',
      key: 'amount',
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('rechargeTime'),
      dataIndex: 'used_at',
      key: 'used_at',
      render: (v: string | null) => <span className="rp-mono">{v || '-'}</span>,
    },
  ];

  const orderColumns = [
    { title: t('orderId'), dataIndex: 'id', key: 'id', width: 70 },
    { title: t('planName'), dataIndex: 'plan_name', key: 'plan_name' },
    {
      title: t('planPrice'),
      dataIndex: 'price',
      key: 'price',
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('purchaseTime'),
      dataIndex: 'created_at',
      key: 'created_at',
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
  ];

  return (
    <Card style={{ marginTop: 24 }}>
      <Tabs
        items={[
          {
            key: 'redeem',
            label: t('rechargeHistory'),
            children: (
              <Table
                dataSource={redeems}
                columns={redeemColumns}
                rowKey="id"
                loading={loading}
                size="small"
                scroll={{ x: 'max-content' }}
                pagination={{ pageSize: 10, showSizeChanger: false }}
                locale={{ emptyText: t('noRecharges') }}
              />
            ),
          },
          {
            key: 'orders',
            label: t('orderHistory'),
            children: (
              <Table
                dataSource={orders}
                columns={orderColumns}
                rowKey="id"
                loading={loading}
                size="small"
                scroll={{ x: 'max-content' }}
                pagination={{ pageSize: 10, showSizeChanger: false }}
                locale={{ emptyText: t('noOrders') }}
              />
            ),
          },
        ]}
      />
    </Card>
  );
}
