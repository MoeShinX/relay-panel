import { Alert } from 'antd';
import { NotificationOutlined } from '@ant-design/icons';
import { useSiteNotice } from '../hooks/useSiteNotice';
import { renderMarkdown } from '../utils/markdown';

/** The four severities the backend allows. Anything else was already coerced
 *  to "info" server-side; this is the last stop before antd's Alert. */
const TYPES = ['info', 'success', 'warning', 'error'] as const;
type AlertType = (typeof TYPES)[number];

/**
 * v1.3.0: the operator's announcement, shown at the top of the dashboard and
 * the account page.
 *
 * Renders nothing at all when the announcement is empty. An operator who never
 * writes one should not see a permanent empty box on their dashboard.
 */
export default function SiteAnnouncement() {
  const { announcement, announcement_type } = useSiteNotice();

  if (!announcement) return null;

  const type: AlertType = (TYPES as readonly string[]).includes(announcement_type)
    ? (announcement_type as AlertType)
    : 'info';

  return (
    <Alert
      type={type}
      showIcon
      icon={<NotificationOutlined />}
      style={{ marginBottom: 16 }}
      // renderMarkdown returns React elements, never an HTML string — see
      // utils/markdown. That is what keeps operator-authored text from
      // becoming markup on every signed-in user's page.
      description={<div style={{ whiteSpace: 'pre-wrap' }}>{renderMarkdown(announcement)}</div>}
    />
  );
}
