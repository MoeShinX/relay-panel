import { Alert } from 'antd';
import { NotificationOutlined } from '@ant-design/icons';
import { useSiteNotice } from '../hooks/useSiteNotice';

/**
 * v1.3.0: the operator's announcement, shown at the top of the dashboard and
 * the account page.
 *
 * Renders nothing at all when the announcement is empty. An operator who never
 * writes one should not see a permanent empty box on their dashboard.
 */
export default function SiteAnnouncement() {
  const { announcement } = useSiteNotice();

  if (!announcement) return null;

  return (
    <Alert
      type="info"
      showIcon
      icon={<NotificationOutlined />}
      style={{ marginBottom: 16 }}
      // whiteSpace preserves the line breaks the operator typed. The text is
      // rendered by React as a string, never as HTML — an announcement is
      // admin-authored, but injecting markup into every user's page is not a
      // capability worth handing out.
      description={<span style={{ whiteSpace: 'pre-wrap' }}>{announcement}</span>}
    />
  );
}
