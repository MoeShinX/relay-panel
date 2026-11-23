import { Layout, Menu, Button, Space, Typography, Segmented, Modal, Form, Input, message, Spin, Badge, Tooltip, Drawer, Dropdown } from 'antd';
import type { MenuProps } from 'antd';
import { Outlet, useNavigate, useLocation } from 'react-router-dom';
import { useEffect, useState, Suspense } from 'react';
import {
  MenuOutlined,
  EllipsisOutlined,
  TranslationOutlined,
  DashboardOutlined,
  ApiOutlined,
  CloudServerOutlined,
  RadarChartOutlined,
  ClusterOutlined,
  UserOutlined,
  LogoutOutlined,
  LockOutlined,
  SettingOutlined,
  ShoppingOutlined,
  TeamOutlined,
  NotificationOutlined,
  GithubOutlined,
} from '@ant-design/icons';
import { useI18n } from '../i18n/context';
import api from '../api/client';
import type { ApiEnvelope } from '../api/types';
import { useAuth } from '../auth/useAuth';
import { useSite } from '../hooks/useSite';
import { useAnnouncementBadge } from '../hooks/useAnnouncementBadge';
import { useIsMobile } from '../hooks/useIsMobile';
import { makePasswordValidator } from '../utils/password';

const { Sider, Content, Header } = Layout;
const { Text } = Typography;

export default function MainLayout() {
  const navigate = useNavigate();
  const location = useLocation();
  const { t, lang, setLang } = useI18n();
  const { isAdmin, user, logout: authLogout } = useAuth();
  const site = useSite();
  // Marked seen by the archive page itself, so the dot clears only once the
  // notices have actually been shown.
  const { unread } = useAnnouncementBadge(user?.id ?? null);
  const [changePwOpen, setChangePwOpen] = useState(false);
  const [pwForm] = Form.useForm();
  const [pwSubmitting, setPwSubmitting] = useState(false);
  // Phones get no sider: the menu slides in from the left on demand, and the
  // header's controls fold into one menu (see the mobile branch below).
  const isMobile = useIsMobile();
  const [menuOpen, setMenuOpen] = useState(false);
  const [siderCollapsed, setSiderCollapsed] = useState(false);

  // v0.4.11 PR2: role-based navigation.
  // Admin: Dashboard → 个人中心, 转发规则, 设备分组, 节点状态, 隧道配置, 用户管理, 系统设置
  // Regular: 个人中心, 我的规则, 可用节点
  // v1.0.7: 仪表盘 (/) is admin-only — the regular-user dashboard was removed
  // (redirects to /account), so regular users no longer get this menu entry.
  const dashboardItem = { key: '/', icon: <DashboardOutlined />, label: t('dashboard') };
  const sharedItems = [
    { key: '/account', icon: <UserOutlined />, label: t('personalCenter') },
    { key: '/shop', icon: <ShoppingOutlined />, label: t('shop') },
    { key: '/rules', icon: <ApiOutlined />, label: t('myRules') },
    { key: '/nodes', icon: <CloudServerOutlined />, label: t('availableNodes') },
  ];
  // v1.2.4: the admin list had grown to seven top-level entries. Grouped into
  // two submenus by what they ARE, not just to shorten the list:
  //
  //   billing  — records you create, edit and delete day to day
  //   system   — configuration you set once, plus the audit trail
  //
  // Plans and redeem codes deliberately did NOT go under 系统设置: generating
  // codes is a routine task, and burying a daily CRUD page inside "system
  // settings" costs a click every time and misfiles it besides.
  const billingChildren = [
    { key: '/users', label: t('users') },
    { key: '/plans', label: t('planManagement') },
    { key: '/redeem-codes', label: t('redeemCodes') },
  ];
  const systemChildren = [
    { key: '/site-settings', label: t('siteSettings') },
    { key: '/settings', label: t('basicSettings') },
    { key: '/notify-settings', label: t('notifySettings') },
    { key: '/announcement-admin', label: t('announcementAdmin') },
    { key: '/audit-log', label: t('auditLog') },
  ];
  const adminOnlyItems = [
    { key: '/latency', icon: <RadarChartOutlined />, label: t('networkMonitor') },
    { key: '/groups', icon: <ClusterOutlined />, label: t('deviceGroups') },
    {
      key: 'grp-billing',
      icon: <TeamOutlined />,
      label: t('userAndBilling'),
      children: billingChildren,
    },
    {
      key: 'grp-system',
      icon: <SettingOutlined />,
      label: t('systemSettings'),
      children: systemChildren,
    },
  ];
  const menuItems = isAdmin
    ? [dashboardItem, ...sharedItems, ...adminOnlyItems]
    : sharedItems;

  // Expand whichever group holds the current page. Synced with route changes
  // while still letting the user manually toggle submenus.
  const routeGroupKey = billingChildren.some((c) => c.key === location.pathname)
    ? 'grp-billing'
    : systemChildren.some((c) => c.key === location.pathname)
      ? 'grp-system'
      : '';
  const [openKeys, setOpenKeys] = useState<string[]>(() => (routeGroupKey ? [routeGroupKey] : []));
  useEffect(() => {
    if (routeGroupKey) {
      setOpenKeys((prev) => (prev.includes(routeGroupKey) ? prev : [...prev, routeGroupKey]));
    }
  }, [routeGroupKey]);

  const isLatencyRoute =
    location.pathname === '/latency'
    || /^\/groups\/[^/]+\/latency$/.test(location.pathname)
    || /^\/nodes\/[^/]+\/latency$/.test(location.pathname);

  const selectedMenuKey = isLatencyRoute
    ? '/latency'
    : location.pathname === '/node-status' || location.pathname.startsWith('/nodes/')
      ? '/nodes'
      : location.pathname.startsWith('/groups/')
        ? '/groups'
        : location.pathname;

  const logout = () => {
    authLogout();
    navigate('/login');
  };

  const handleChangePassword = async (values: { current_password: string; new_password: string }) => {
    setPwSubmitting(true);
    try {
      const res = await api.put<unknown, ApiEnvelope<null>>('/user/password', values);
      if (res.code !== 0) {
        message.error(res.message);
        return;
      }
      message.success(t('passwordChanged'));
      setChangePwOpen(false);
      pwForm.resetFields();
      authLogout();
      navigate('/login', { replace: true });
    } catch {
      message.error(t('passwordChangeFailed'));
    } finally {
      setPwSubmitting(false);
    }
  };

  const brand = site.site_name || t('brand');
  const brandMark = (size = 24) => (
    <svg
      aria-hidden="true"
      width={size}
      height={size}
      viewBox="0 0 64 64"
      fill="none"
      style={{ flexShrink: 0 }}
    >
      <defs>
        <linearGradient id="rp-logo-grad" x1="8" y1="5" x2="57" y2="61" gradientUnits="userSpaceOnUse">
          <stop stopColor="#818CF8" />
          <stop offset="1" stopColor="#4F46E5" />
        </linearGradient>
      </defs>
      <rect x="3" y="3" width="58" height="58" rx="16" fill="url(#rp-logo-grad)" />
      <path
        d="M15 23h27m0 0-7-7m7 7-7 7M49 41H22m0 0 7 7m-7-7 7-7"
        stroke="#FFFFFF"
        strokeWidth="6"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );

  // The drawer and the sider keep an operator's long site name inside their
  // 52px: a margin each side, at most two lines, then an ellipsis (full name
  // on hover). The collapsed sider is 80px, narrower than RelayPanel itself
  // at 17px, so there the name drops to 13px.
  const brandBlock = (collapsed = false) => (
    <div title={brand} style={{
      height: 'var(--rp-header-height)',
      display: 'flex', alignItems: 'center', justifyContent: collapsed ? 'center' : 'flex-start',
      gap: collapsed ? 0 : 10,
      padding: collapsed ? '0 4px' : '0 18px',
      borderBottom: '1px solid var(--rp-sidebar-border, var(--rp-border))',
      marginBottom: 6,
      color: '#09090b', fontSize: collapsed ? 13 : 17, fontWeight: 600, letterSpacing: collapsed ? 0 : -0.2,
    }}>
      {!collapsed && brandMark(24)}
      <span style={{
        display: '-webkit-box', WebkitBoxOrient: 'vertical', WebkitLineClamp: 2,
        overflow: 'hidden', overflowWrap: 'anywhere', textAlign: collapsed ? 'center' : 'left', lineHeight: 1.3,
      }}>
        {brand}
      </span>
    </div>
  );
  // One menu for both homes: the desktop sider and the phone drawer.
  const menu = (
    <Menu
      theme="light"
      mode="inline"
      selectedKeys={[selectedMenuKey]}
      openKeys={siderCollapsed && !isMobile ? undefined : openKeys}
      onOpenChange={(keys) => setOpenKeys(keys as string[])}
      items={menuItems}
      // Parent entries carry a `grp-` key and no route — clicking one only
      // expands it, so navigating on them would 404.
      onClick={({ key }) => {
        if (key.startsWith('grp-')) return;
        navigate(key);
        setMenuOpen(false);
      }}
      style={{ borderRight: 0, background: 'transparent' }}
    />
  );

  // The phone header has room for a bell and one menu; everything the desktop
  // header spreads across a row lives here. The first row says who is signed
  // in, which the desktop header shows as the role text.
  const moreItems: MenuProps['items'] = [
    { key: 'who', label: `${user?.username ?? ''} · ${isAdmin ? t('admin') : t('user')}`, disabled: true },
    { type: 'divider' },
    { key: 'lang', icon: <TranslationOutlined />, label: lang === 'zh-CN' ? t('langEnUS') : t('langZhCN') },
    { key: 'password', icon: <LockOutlined />, label: t('changePassword') },
    { key: 'github', icon: <GithubOutlined />, label: 'GitHub' },
    { type: 'divider' },
    { key: 'logout', icon: <LogoutOutlined />, label: t('logout'), danger: true },
  ];
  const onMore: MenuProps['onClick'] = ({ key }) => {
    if (key === 'lang') setLang(lang === 'zh-CN' ? 'en-US' : 'zh-CN');
    else if (key === 'password') setChangePwOpen(true);
    else if (key === 'github') window.open('https://github.com/MoeShinX/relay-panel', '_blank', 'noopener,noreferrer');
    else if (key === 'logout') logout();
  };

  const userMenuItems: MenuProps['items'] = [
    { key: 'who', label: `${user?.username ?? ''} · ${isAdmin ? t('admin') : t('user')}`, disabled: true },
    { type: 'divider' },
    { key: 'account', icon: <UserOutlined />, label: t('personalCenter') },
    { key: 'password', icon: <LockOutlined />, label: t('changePassword') },
    { type: 'divider' },
    { key: 'logout', icon: <LogoutOutlined />, label: t('logout'), danger: true },
  ];
  const onUserMenu: MenuProps['onClick'] = ({ key }) => {
    if (key === 'account') navigate('/account');
    else if (key === 'password') setChangePwOpen(true);
    else if (key === 'logout') logout();
  };
  const userInitial = (user?.username?.trim()?.[0] || 'U').toUpperCase();

  const flatNavLabels: Record<string, string> = {
    '/': t('dashboard'),
    '/account': t('personalCenter'),
    '/shop': t('shop'),
    '/rules': t('myRules'),
    '/nodes': t('availableNodes'),
    '/latency': t('networkMonitor'),
    '/groups': t('deviceGroups'),
    '/users': t('users'),
    '/plans': t('planManagement'),
    '/redeem-codes': t('redeemCodes'),
    '/site-settings': t('siteSettings'),
    '/settings': t('basicSettings'),
    '/notify-settings': t('notifySettings'),
    '/announcement-admin': t('announcementAdmin'),
    '/audit-log': t('auditLog'),
    '/announcements': t('announcements'),
  };
  const currentSectionLabel = flatNavLabels[selectedMenuKey] || '';

  return (
    <Layout style={{ minHeight: '100vh' }}>
      {isMobile ? (
        <Drawer
          placement="left"
          open={menuOpen}
          onClose={() => setMenuOpen(false)}
          size={240}
          closable={false}
          styles={{ body: { padding: 0, background: 'var(--rp-sidebar-bg)' } }}
        >
          {brandBlock()}
          {menu}
        </Drawer>
      ) : (
        <Sider
          collapsible
          collapsed={siderCollapsed}
          breakpoint="lg"
          width={224}
          onCollapse={(collapsed) => setSiderCollapsed(collapsed)}
          style={{
            background: 'var(--rp-sidebar-bg)',
            position: 'sticky', top: 0, height: '100vh', overflow: 'auto',
          }}
        >
          {brandBlock(siderCollapsed)}
          {menu}
        </Sider>
      )}
      <Layout>
        <Header style={{
          position: 'sticky', top: 0, zIndex: 100,
          background: 'rgba(255, 255, 255, 0.88)',
          backdropFilter: 'blur(12px)',
          WebkitBackdropFilter: 'blur(12px)',
          height: 'var(--rp-header-height)',
          padding: isMobile ? '0 8px 0 4px' : '0 24px', lineHeight: 'var(--rp-header-height)',
          display: 'flex', justifyContent: 'space-between', alignItems: 'center',
          borderBottom: '1px solid var(--rp-border)',
        }}>
          {isMobile ? (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 6, flex: '1 1 auto', minWidth: 0 }}>
                <Button type="text" icon={<MenuOutlined />} aria-label={t('openMenu')} onClick={() => setMenuOpen(true)} style={{ flexShrink: 0 }} />
                <Text strong ellipsis={{ tooltip: brand }} style={{ fontSize: 16, minWidth: 0 }}>{brand}</Text>
              </div>
              <Space size={4} style={{ flexShrink: 0 }}>
                <Badge dot={unread} offset={[-6, 6]}>
                  <Button type="text" icon={<NotificationOutlined />} aria-label={t('announcements')} onClick={() => navigate('/announcements')} />
                </Badge>
                <Dropdown menu={{ items: moreItems, onClick: onMore }} trigger={['click']} placement="bottomRight">
                  <Button type="text" icon={<EllipsisOutlined />} aria-label={t('moreActions')} />
                </Dropdown>
              </Space>
            </>
          ) : (
            <>
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, minWidth: 0 }}>
                {currentSectionLabel && (
                  <Text style={{ fontSize: 13, color: 'var(--rp-text-secondary)', fontWeight: 500 }}>
                    {currentSectionLabel}
                  </Text>
                )}
              </div>
              <Space size={12}>
                <Badge dot={unread} offset={[-4, 2]}>
                  <Button
                    type="text"
                    size="small"
                    icon={<NotificationOutlined />}
                    onClick={() => navigate('/announcements')}
                  >
                    {t('announcements')}
                  </Button>
                </Badge>
                <Segmented
                  size="small"
                  value={lang}
                  onChange={(v) => setLang(v as 'zh-CN' | 'en-US')}
                  options={[
                    { value: 'zh-CN', label: t('langZhCN') },
                    { value: 'en-US', label: t('langEnUS') },
                  ]}
                />
                <Tooltip title="GitHub">
                  <Button
                    type="text"
                    size="small"
                    icon={<GithubOutlined />}
                    href="https://github.com/MoeShinX/relay-panel"
                    target="_blank"
                    rel="noopener noreferrer"
                    aria-label="GitHub"
                  />
                </Tooltip>
                <Dropdown menu={{ items: userMenuItems, onClick: onUserMenu }} trigger={['click']} placement="bottomRight">
                  <button type="button" className="rp-user-pill">
                    <span className="rp-user-avatar">{userInitial}</span>
                    <span>{user?.username || (isAdmin ? t('admin') : t('user'))}</span>
                    <span style={{
                      fontSize: 11,
                      padding: '1px 6px',
                      borderRadius: 999,
                      background: '#f4f4f5',
                      color: '#52525b',
                      border: '1px solid #e4e4e7',
                    }}>
                      {isAdmin ? t('admin') : t('user')}
                    </span>
                  </button>
                </Dropdown>
              </Space>
            </>
          )}
        </Header>
        <Content style={{ margin: 'var(--rp-content-padding)', background: 'var(--rp-bg)' }}>
          {/* v1.2 (PR4): lazy-loaded pages (router.tsx) suspend here on first
              navigation to their chunk, showing a centered spinner instead of a
              blank pane. */}
          <Suspense fallback={<div style={{ textAlign: 'center', padding: 48 }}><Spin /></div>}>
            <div className="rp-main-inner">
              <Outlet />
            </div>
          </Suspense>
        </Content>
      </Layout>

      <Modal
        title={t('changePassword')}
        open={changePwOpen}
        onCancel={() => { setChangePwOpen(false); pwForm.resetFields(); }}
        onOk={() => pwForm.submit()}
        okText={t('save')}
        cancelText={t('cancel')}
        confirmLoading={pwSubmitting}
      >
        <Form form={pwForm} onFinish={handleChangePassword} layout="vertical">
          <Form.Item
            name="current_password"
            label={t('currentPassword')}
            rules={[{ required: true }]}
          >
            <Input.Password autoComplete="current-password" />
          </Form.Item>
          <Form.Item
            name="new_password"
            label={t('newPassword')}
            rules={[
              { required: true, message: t('passwordRequired') },
              { validator: makePasswordValidator(t('newPasswordTooShort'), t('passwordTooLong')) },
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
          <Form.Item
            name="confirm_password"
            label={t('confirmPassword')}
            dependencies={['new_password']}
            rules={[
              { required: true },
              ({ getFieldValue }) => ({
                validator(_, value) {
                  if (!value || getFieldValue('new_password') === value) {
                    return Promise.resolve();
                  }
                  return Promise.reject(new Error(t('passwordsDoNotMatch')));
                },
              }),
            ]}
          >
            <Input.Password autoComplete="new-password" />
          </Form.Item>
        </Form>
      </Modal>
    </Layout>
  );
}
