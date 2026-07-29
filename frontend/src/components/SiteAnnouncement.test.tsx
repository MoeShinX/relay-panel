import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import SiteAnnouncement from './SiteAnnouncement';
import { invalidateActiveAnnouncement } from '../hooks/useActiveAnnouncement';
import type { Announcement } from '../api/types';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

const ann = (over: Partial<Announcement> = {}): Announcement => ({
  id: 1,
  title: '',
  content: '今晚 02:00 维护',
  kind: 'warning',
  pinned: false,
  published_at: '2026-07-28 10:00:00',
  expires_at: null,
  author_id: 1,
  author_name: 'admin',
  ...over,
});

beforeEach(() => {
  mockGet.mockReset();
  // The hook caches module-wide; without this each test would see the first
  // test's response.
  invalidateActiveAnnouncement();
});

// The banner links to the archive, so it needs a router in tests.
const renderBanner = async () => {
  await act(async () => {
    render(
      <MemoryRouter>
        <SiteAnnouncement />
      </MemoryRouter>,
    );
  });
};

describe('SiteAnnouncement', () => {
  it('renders the active announcement', async () => {
    mockGet.mockResolvedValue(ok(ann()));
    await renderBanner();
    expect(screen.getByText('今晚 02:00 维护')).toBeInTheDocument();
    expect(document.querySelector('.ant-alert-warning')).not.toBeNull();
  });

  it('reads the active endpoint, not the site config', async () => {
    // The announcement moved out of site:config into its own table; reading the
    // old source would serve text frozen at migration time.
    mockGet.mockResolvedValue(ok(ann()));
    await renderBanner();
    expect(mockGet.mock.calls[0][0]).toBe('/user/announcements/active');
  });

  it('renders nothing when there is no live announcement', async () => {
    // The default state for a fresh install, and what an expired notice leaves
    // behind — a permanent empty box would be worse than no banner.
    mockGet.mockResolvedValue(ok(null));
    const { container } = await act(async () =>
      render(<MemoryRouter><SiteAnnouncement /></MemoryRouter>),
    ) as unknown as { container: HTMLElement };
    expect(container.querySelector('.ant-alert')).toBeNull();
  });

  it('stays silent when the request fails', async () => {
    mockGet.mockRejectedValue(new Error('network'));
    const { container } = await act(async () =>
      render(<MemoryRouter><SiteAnnouncement /></MemoryRouter>),
    ) as unknown as { container: HTMLElement };
    expect(container.querySelector('.ant-alert')).toBeNull();
  });

  it('falls back to info for an unknown severity', async () => {
    // The backend coerces this already; the component must not pass an unknown
    // value to antd, where it renders unstyled.
    mockGet.mockResolvedValue(ok(ann({ kind: 'chartreuse' })));
    await renderBanner();
    expect(document.querySelector('.ant-alert-info')).not.toBeNull();
  });

  it('renders the markdown subset and shows who posted it', async () => {
    mockGet.mockResolvedValue(ok(ann({ content: '**加粗** 和 [链接](https://example.com)' })));
    await renderBanner();
    expect(document.querySelector('.ant-alert strong')?.textContent).toBe('加粗');
    expect(document.querySelector('.ant-alert a[href="https://example.com"]')).not.toBeNull();
    expect(screen.getByText(/admin/)).toBeInTheDocument();
  });

  it('never renders the announcement as HTML', async () => {
    // Admin-authored, but injecting markup into every user's page is not a
    // capability this field should hand out.
    mockGet.mockResolvedValue(ok(ann({ content: '<img src=x onerror=alert(1)>' })));
    await renderBanner();
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
    expect(document.querySelector('.ant-alert img')).toBeNull();
  });
});
