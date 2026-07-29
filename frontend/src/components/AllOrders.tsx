import { Card, Table, Typography } from 'antd';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';
import { useI18n } from '../i18n/context';

const { Text } = Typography;

const PAGE_SIZE = 20;

/** One row of the operator-wide purchase list. */
interface AdminOrderRow {
  id: number;
  user_id: number;
  /** null when the buyer's account was deleted — the order survives as the
   *  money-in record, so the id is kept and only the name goes missing. */
  username: string | null;
  plan_name: string;
  price: string;
  created_at: string;
}

/**
 * v1.3.0: every user's purchases, under the plan-management table.
 *
 * The shop page shows a user their own orders; this is the operator's view of
 * all of them, which is what answers "who bought what this week". Paginated
 * because this table only grows — the per-user list it mirrors is naturally
 * small enough not to need it.
 */
export default function AllOrders() {
  const { t } = useI18n();
  const [items, setItems] = useState<AdminOrderRow[]>([]);
  const [total, setTotal] = useState(0);
  const [page, setPage] = useState(1);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const qs = new URLSearchParams({
        limit: String(PAGE_SIZE),
        offset: String((page - 1) * PAGE_SIZE),
      });
      const res = await api.get<unknown, ApiEnvelope<{ items: AdminOrderRow[]; total: number }>>(
        `/admin/orders?${qs}`,
      );
      if (res.code === 0 && res.data) {
        setItems(res.data.items);
        setTotal(res.data.total);
      }
    } catch {
      // An unreachable list shows its empty state rather than an error box on
      // a page whose main job is managing plans.
    } finally {
      setLoading(false);
    }
  }, [page]);

  useEffect(() => { load(); }, [load]);

  const columns = [
    { title: t('orderId'), dataIndex: 'id', key: 'id', width: 80 },
    {
      title: t('orderUser'),
      key: 'user',
      width: 160,
      // Falls back to the id so a deleted account still identifies the buyer
      // as far as the data allows.
      render: (_: unknown, r: AdminOrderRow) =>
        r.username ?? <Text type="secondary">#{r.user_id}</Text>,
    },
    { title: t('planName'), dataIndex: 'plan_name', key: 'plan_name' },
    {
      title: t('planPrice'),
      dataIndex: 'price',
      key: 'price',
      width: 110,
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
    {
      title: t('purchaseTime'),
      dataIndex: 'created_at',
      key: 'created_at',
      width: 180,
      render: (v: string) => <span className="rp-mono">{v}</span>,
    },
  ];

  return (
    <Card title={t('allOrders')} style={{ marginTop: 24 }}>
      <Table
        rowKey="id"
        loading={loading}
        columns={columns}
        dataSource={items}
        size="small"
        scroll={{ x: 'max-content' }}
        locale={{ emptyText: t('noOrdersAll') }}
        pagination={{
          current: page,
          pageSize: PAGE_SIZE,
          total,
          showSizeChanger: false,
          onChange: setPage,
        }}
      />
    </Card>
  );
}
