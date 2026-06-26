import type { ThemeConfig } from 'antd';

// RelayPanel — "石墨靛蓝 / Graphite + Indigo" theme.
// Token-driven: every antd component (buttons, tables, tags, inputs, menu…)
// picks this up via ConfigProvider, so we don't touch individual components.
// Layout chrome (sidebar bg, radii, custom tags) lives in styles/theme.css.
export const rpTheme: ThemeConfig = {
  token: {
    colorPrimary: '#6366f1',
    colorInfo: '#6366f1',
    colorSuccess: '#059669',
    colorWarning: '#d97706',
    colorError: '#ef4444',
    colorLink: '#6366f1',
    borderRadius: 8,
    colorBgLayout: '#fafafa',
    colorBorderSecondary: '#ececef',
    fontWeightStrong: 500,
    fontFamily:
      '"Noto Sans SC", system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", ' +
      '"PingFang SC", "Microsoft YaHei", "Helvetica Neue", Arial, sans-serif',
  },
  components: {
    // Dark graphite sidebar: transparent items so the Sider's #18181b shows
    // through, indigo-tinted selection instead of antd's solid blue bar.
    Menu: {
      darkItemBg: 'transparent',
      darkSubMenuItemBg: 'transparent',
      darkItemColor: '#a1a1aa',
      darkItemHoverColor: '#fafafa',
      darkItemHoverBg: 'rgba(255,255,255,0.05)',
      darkItemSelectedBg: 'rgba(99,102,241,0.18)',
      darkItemSelectedColor: '#c7d2fe',
    },
    // Flat buttons — drop antd's default control shadow for a cleaner look.
    Button: {
      primaryShadow: 'none',
      defaultShadow: 'none',
      dangerShadow: 'none',
    },
    Card: {
      borderRadiusLG: 12,
    },
    Table: {
      headerBg: '#fafafa',
      rowHoverBg: '#f5f5ff',
      borderColor: '#ececef',
    },
    Layout: {
      bodyBg: '#fafafa',
      headerBg: '#ffffff',
    },
  },
};
