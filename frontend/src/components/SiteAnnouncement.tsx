import { Alert, Typography } from 'antd';
import { NotificationOutlined } from '@ant-design/icons';
import { Link } from 'react-router-dom';
import { useActiveAnnouncement } from '../hooks/useActiveAnnouncement';
import { useI18n } from '../i18n/context';
import { renderMarkdown } from '../utils/markdown';

const { Text } = Typography;

/** The four severities the backend allows. Anything else was already coerced
 *  server-side; this is the last stop before antd's Alert. */
const TYPES = ['info', 'success', 'warning', 'error'] as const;
type AlertType = (typeof TYPES)[number];

/**
 * v1.3.0: the operator's current announcement, at the top of the dashboard and
 * the account page.
 *
 * Shows ONE notice — the pinned one, else the newest unexpired one. Stacking
 * several banners would push the actual page content off the screen and train
 * people to ignore all of them. Past notices live on the archive page, linked
 * from here.
 *
 * Renders nothing at all when there is no live notice.
 */
export default function SiteAnnouncement() {
  const { t } = useI18n();
  const active = useActiveAnnouncement();

  if (!active) return null;

  const type: AlertType = (TYPES as readonly string[]).includes(active.kind)
    ? (active.kind as AlertType)
    : 'info';

  return (
    <Alert
      type={type}
      showIcon
      icon={<NotificationOutlined />}
      style={{ marginBottom: 16 }}
      // antd v6 renamed Alert's `message` prop to `title`; `message` is
      // silently ignored, which is how the heading first came out blank.
      title={active.title || undefined}
      description={
        <div>
          {/* renderMarkdown returns React elements, never an HTML string — see
              utils/markdown. That is what keeps operator-authored text from
              becoming markup on every signed-in user's page. */}
          <div style={{ whiteSpace: 'pre-wrap' }}>{renderMarkdown(active.content)}</div>
          <div style={{ marginTop: 6 }}>
            <Text type="secondary" style={{ fontSize: 12 }}>
              {active.published_at}
              {active.author_name ? ` · ${active.author_name}` : ''}
            </Text>
            <Link to="/announcements" style={{ fontSize: 12, marginLeft: 12 }}>
              {t('viewAllAnnouncements')}
            </Link>
          </div>
        </div>
      }
    />
  );
}
