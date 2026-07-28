import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, act } from '@testing-library/react';

const { mockGet } = vi.hoisted(() => ({ mockGet: vi.fn() }));
vi.mock('../api/client', () => ({ default: { get: mockGet } }));

import SiteAnnouncement from './SiteAnnouncement';
import { invalidateSiteNotice } from '../hooks/useSiteNotice';

const ok = <T,>(data: T) => ({ code: 0, message: 'ok', data });

beforeEach(() => {
  mockGet.mockReset();
  // The hook caches module-wide; without this each test would see the first
  // test's response.
  invalidateSiteNotice();
});

const renderBanner = async () => { await act(async () => { render(<SiteAnnouncement />); }); };

describe('SiteAnnouncement', () => {
  it('renders the announcement text when the operator set one', async () => {
    mockGet.mockResolvedValue(ok({ announcement: '今晚 02:00 维护', contact: '' }));
    await renderBanner();
    expect(screen.getByText('今晚 02:00 维护')).toBeInTheDocument();
  });

  it('renders nothing at all when the announcement is empty', async () => {
    // An operator who never writes one must not get a permanent empty box —
    // this is the default state for every fresh install.
    mockGet.mockResolvedValue(ok({ announcement: '', contact: 'tg:@ops' }));
    const { container } = await act(async () => render(<SiteAnnouncement />)) as unknown as { container: HTMLElement };
    expect(container).toBeEmptyDOMElement();
  });

  it('stays silent when the request fails', async () => {
    // A dashboard should not sprout an error box because an optional banner
    // could not be fetched.
    mockGet.mockRejectedValue(new Error('network'));
    const { container } = await act(async () => render(<SiteAnnouncement />)) as unknown as { container: HTMLElement };
    expect(container).toBeEmptyDOMElement();
  });

  it('never renders the announcement as HTML', async () => {
    // Admin-authored, but injecting markup into every user's page is not a
    // capability this field should hand out.
    mockGet.mockResolvedValue(ok({ announcement: '<img src=x onerror=alert(1)>', contact: '' }));
    await renderBanner();
    expect(screen.getByText('<img src=x onerror=alert(1)>')).toBeInTheDocument();
    expect(document.querySelector('img')).toBeNull();
  });
});
