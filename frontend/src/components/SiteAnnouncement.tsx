import { Alert, Button, Typography } from 'antd';
import { NotificationOutlined, RightOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useActiveAnnouncement } from '../hooks/useActiveAnnouncement';
import { useI18n } from '../i18n/context';
import { stripMarkdown } from '../utils/markdown';

const { Paragraph } = Typography;

/** The four severities the backend allows. Anything else was already coerced
 *  server-side; this is the last stop before antd's Alert. */
const TYPES = ['info', 'success', 'warning', 'error'] as const;
type AlertType = (typeof TYPES)[number];

/**
 * v1.2.4: a one-glance summary of the current announcement.
 *
 * Deliberately NOT the full text. Carrying the whole notice made this 299px —
 * a third of the viewport — for a routine maintenance message, pushing the
 * account details the page exists for below the fold. A banner is a prompt;
 * the archive page is the reading surface.
 *
 * So: title, two clamped lines of plain text, and a link. The body is stripped
 * of markup rather than rendered, because a two-line excerpt with half a bold
 * run and a dangling list dash reads worse than the same words plain — and the
 * formatting is intact one click away.
 *
 * Renders nothing at all when there is no live notice.
 */
export default function SiteAnnouncement() {
  const { t } = useI18n();
  const navigate = useNavigate();
  const active = useActiveAnnouncement();

  if (!active) return null;

  const type: AlertType = (TYPES as readonly string[]).includes(active.kind)
    ? (active.kind as AlertType)
    : 'info';

  const summary = stripMarkdown(active.content);

  return (
    <Alert
      type={type}
      showIcon
      icon={<NotificationOutlined />}
      style={{ marginBottom: 16 }}
      title={active.title || undefined}
      description={
        <Paragraph
          type="secondary"
          // antd's own multi-line ellipsis. A hand-rolled -webkit-line-clamp
          // did not survive to the DOM here — the other two clamp properties
          // landed in the inline style and this one was dropped — so rather
          // than fight whatever swallows it, use the API built for this. It is
          // what bounds the banner: a 4000-character notice takes exactly as
          // much room as a short one.
          ellipsis={{ rows: 2, tooltip: summary }}
          style={{ marginBottom: 0 }}
        >
          {summary}
        </Paragraph>
      }
      // `action` keeps the link out of the text flow, so it cannot be pushed
      // around by the length of the summary.
      action={
        <Button type="link" size="small" onClick={() => navigate('/announcements')}>
          {t('viewAnnouncementDetail')} <RightOutlined />
        </Button>
      }
    />
  );
}
