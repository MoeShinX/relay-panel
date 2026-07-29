import { Card, List, Tag, Empty, Spin, Typography, Pagination } from 'antd';
import { useCallback, useEffect, useState } from 'react';
import api from '../api/client';
import type { ApiEnvelope, Announcement, AnnouncementList } from '../api/types';
import { useI18n } from '../i18n/context';
import { renderMarkdown } from '../utils/markdown';
import { useAnnouncementBadge } from '../hooks/useAnnouncementBadge';
import { useAuth } from '../auth/useAuth';
import { kindLabel } from '../utils/announcementKind';

const { Text, Title } = Typography;

const PAGE_SIZE = 10;

const TAG_COLOR: Record<string, string> = {
  info: 'blue',
  success: 'green',
  warning: 'orange',
  error: 'red',
};

/**
 * v1.2.4: the announcement archive, for any signed-in user.
 *
 * The banner only ever shows one notice, so without this page everything the
 * operator ever announced was unreadable the moment the next one went up.
 * Expired notices are included deliberately — reading what was announced last
 * month is the entire point.
 */
export default function Announcements() {
  const { t } = useI18n();
  const { user } = useAuth();
  const { latestId, markSeen } = useAnnouncementBadge(user?.id ?? null);
  const [items, setItems] = useState<Announcement[]>([]);
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
      const res = await api.get<unknown, ApiEnvelope<AnnouncementList>>(
        `/user/announcements?${qs}`,
      );
      if (res.code === 0 && res.data) {
        setItems(res.data.items);
        setTotal(res.data.total);
      }
    } catch {
      // An unreachable archive shows the empty state rather than an error box.
    } finally {
      setLoading(false);
    }
  }, [page]);

  useEffect(() => { load(); }, [load]);

  // Clear the header dot once the archive has actually rendered — marking on
  // click would clear it even if the page then failed to load.
  useEffect(() => {
    if (latestId > 0) markSeen();
  }, [latestId, markSeen]);

  const isExpired = (a: Announcement) =>
    !!a.expires_at && a.expires_at <= new Date().toISOString().slice(0, 19).replace('T', ' ');

  return (
    <Card title={t('announcements')}>
      <Spin spinning={loading}>
        {items.length === 0 && !loading ? (
          <Empty description={t('noAnnouncements')} image={Empty.PRESENTED_IMAGE_SIMPLE} />
        ) : (
          <>
            <List
              itemLayout="vertical"
              dataSource={items}
              renderItem={(a) => (
                <List.Item key={a.id}>
                  <div style={{ marginBottom: 6 }}>
                    <Tag color={TAG_COLOR[a.kind] ?? 'blue'}>{kindLabel(t, a.kind)}</Tag>
                    {a.pinned && <Tag color="gold">{t('pinned')}</Tag>}
                    {isExpired(a) && <Tag>{t('expired')}</Tag>}
                    {a.title && (
                      <Title level={5} style={{ display: 'inline', marginLeft: 4 }}>
                        {a.title}
                      </Title>
                    )}
                  </div>
                  {/* Same renderer as the banner — React elements, never HTML. */}
                  <div style={{ whiteSpace: 'pre-wrap' }}>{renderMarkdown(a.content)}</div>
                  <div style={{ marginTop: 8 }}>
                    <Text type="secondary" style={{ fontSize: 12 }}>
                      {a.published_at}
                      {a.author_name ? ` · ${a.author_name}` : ''}
                    </Text>
                  </div>
                </List.Item>
              )}
            />
            {total > PAGE_SIZE && (
              <Pagination
                current={page}
                pageSize={PAGE_SIZE}
                total={total}
                showSizeChanger={false}
                onChange={setPage}
                style={{ marginTop: 12, textAlign: 'right' }}
              />
            )}
          </>
        )}
      </Spin>
    </Card>
  );
}
