import { describe, expect, it, vi, beforeEach } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { MemoryRouter, Route, Routes } from 'react-router-dom';

vi.mock('../api/client', () => ({ default: { get: vi.fn(), put: vi.fn() } }));
const { mockLogout } = vi.hoisted(() => ({ mockLogout: vi.fn() }));
vi.mock('../auth/useAuth', () => ({
  useAuth: () => ({ isAdmin: true, user: { id: 1, username: 'admin' }, logout: mockLogout }),
}));
const { mockSite } = vi.hoisted(() => ({ mockSite: vi.fn(() => ({ site_name: 'RelayPanel', subtitle: '' })) }));
vi.mock('../hooks/useSite', () => ({ useSite: mockSite }));
vi.mock('../hooks/useAnnouncementBadge', () => ({
  useAnnouncementBadge: () => ({ latestId: 0, unread: false, markSeen: vi.fn() }),
}));
const { mockIsMobile } = vi.hoisted(() => ({ mockIsMobile: vi.fn(() => false) }));
vi.mock('../hooks/useIsMobile', () => ({ useIsMobile: mockIsMobile }));

import MainLayout from './MainLayout';

// t() echoes keys here (default i18n context), so labels are matched by key.
const renderAt = (path = '/') =>
  render(
    <MemoryRouter initialEntries={[path]}>
      <Routes>
        <Route path="/" element={<MainLayout />}>
          <Route index element={<div>home page</div>} />
          <Route path="rules" element={<div>rules page</div>} />
          <Route path="login" element={<div>login page</div>} />
        </Route>
      </Routes>
    </MemoryRouter>,
  );

beforeEach(() => {
  mockIsMobile.mockReset();
  mockLogout.mockReset();
  mockSite.mockReturnValue({ site_name: 'RelayPanel', subtitle: '' });
});

describe('MainLayout on a desktop', () => {
  it('keeps the sider and the labelled header controls', () => {
    mockIsMobile.mockReturnValue(false);
    const { container } = renderAt();
    expect(container.querySelector('.ant-layout-sider')).not.toBeNull();
    expect(screen.queryByRole('button', { name: 'openMenu' })).toBeNull();
    // the announcements control carries its label as visible text
    expect(screen.getByText('announcements')).toBeInTheDocument();
  });

  // A long site name wrapped down over the menu: 59 characters stood 91px tall
  // in the open sider's 56px brand block, and six lines in the collapsed one.
  it('clamps a long site name in the sider, open or collapsed', async () => {
    const long = '星河科技 RelayPanel 中转加速服务面板 香港日本美国专线';
    mockIsMobile.mockReturnValue(false);
    mockSite.mockReturnValue({ site_name: long, subtitle: '' });
    const user = userEvent.setup();
    const { container } = renderAt();
    const sider = container.querySelector('.ant-layout-sider') as HTMLElement;
    const brandOf = () => within(sider).getByTitle(long);
    const clampOf = () => brandOf().querySelector('span') as HTMLElement;
    expect(clampOf().style.webkitLineClamp).toBe('2');
    expect(brandOf().style.fontSize).toBe('17px');

    await user.click(sider.querySelector('.ant-layout-sider-trigger') as HTMLElement);
    await waitFor(() => expect(sider.className).toContain('ant-layout-sider-collapsed'));
    expect(clampOf().style.webkitLineClamp).toBe('2');
    // 80px is narrower than RelayPanel at 17px
    expect(brandOf().style.fontSize).toBe('13px');
  });
});

// v1.2.13: phones get no sider — it took a fifth of a 375px screen even
// collapsed — and a header that no longer runs off the edge.
describe('MainLayout on a phone', () => {
  it('has no sider; the menu opens from a header button and closes on navigation', async () => {
    mockIsMobile.mockReturnValue(true);
    const user = userEvent.setup();
    const { container } = renderAt();
    expect(container.querySelector('.ant-layout-sider')).toBeNull();
    expect(screen.queryByText('myRules')).toBeNull();

    await user.click(screen.getByRole('button', { name: 'openMenu' }));
    await user.click(await screen.findByText('myRules'));

    expect(await screen.findByText('rules page')).toBeInTheDocument();
    await waitFor(() => expect(container.ownerDocument.querySelector('.ant-drawer-open')).toBeNull());
  });

  it('folds the header controls into one menu that says who is signed in', async () => {
    mockIsMobile.mockReturnValue(true);
    const user = userEvent.setup();
    renderAt();
    // no labelled announcements text in the header, just the bell
    expect(screen.queryByText('announcements')).toBeNull();
    expect(screen.getByRole('button', { name: 'announcements' })).toBeInTheDocument();

    await user.click(screen.getByRole('button', { name: 'moreActions' }));
    expect(await screen.findByText('admin · admin')).toBeInTheDocument();
    expect(screen.getByText('changePassword')).toBeInTheDocument();

    await user.click(screen.getByText('logout'));
    expect(mockLogout).toHaveBeenCalled();
  });

  // The site name is the operator's. Wrapped in the phone header, each line
  // took the header's 56px line height and burst the bar; in the drawer it ran
  // edge to edge and could spill onto the menu.
  it('keeps a long site name to one line in the header and inside the drawer', async () => {
    const long = '星河科技 RelayPanel 中转加速服务面板 香港日本美国专线';
    mockIsMobile.mockReturnValue(true);
    mockSite.mockReturnValue({ site_name: long, subtitle: '' });
    const user = userEvent.setup();
    const { container } = renderAt();

    const headerBrand = within(container.querySelector('.ant-layout-header') as HTMLElement)
      .getByText(long)
      .closest('.ant-typography') as HTMLElement;
    expect(headerBrand.className).toContain('ant-typography-ellipsis');

    await user.click(screen.getByRole('button', { name: 'openMenu' }));
    const drawerBrand = await screen.findByTitle(long);
    expect(drawerBrand.closest('.ant-drawer')).not.toBeNull();
  });
});
