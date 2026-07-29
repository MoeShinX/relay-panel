import { Card, Table } from 'antd';
import { useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, MyRedeemRecord } from '../api/types';
import { useI18n } from '../i18n/context';

/**
 * v1.2.4: the user's own redeem-code top-ups, on the account page.
 *
 * Purchase history is deliberately NOT here — it lives on the shop page, where
 * buying and seeing the receipt happen in the same place. Showing the same
 * table twice only made the account page longer.
 */
export default function RedeemHistory() {
  const { t } = useI18n();
  const [rows, setRows] = useState<MyRedeemRecord[]>([]);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let alive = true;
    api
      .get<unknown, ApiEnvelope<MyRedeemRecord[]>>('/user/redeem-records')
      .then((res) => {
        if (alive && res.code === 0) setRows(res.data ?? []);
      })
      // An empty history and an unreachable one look the same here on purpose:
      // neither is worth an error toast on the landing page.
      .catch(() => {})
      .finally(() => { if (alive) setLoading(false); });
    return () => { alive = false; };
  }, []);

  const columns = [
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

  return (
    <Card title={t('rechargeHistory')} style={{ marginTop: 24 }}>
      <Table
        dataSource={rows}
        columns={columns}
        rowKey="id"
        loading={loading}
        size="small"
        scroll={{ x: 'max-content' }}
        pagination={{ pageSize: 10, showSizeChanger: false }}
        locale={{ emptyText: t('noRecharges') }}
      />
    </Card>
  );
}
