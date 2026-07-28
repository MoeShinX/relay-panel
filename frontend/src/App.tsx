import { useEffect } from 'react';
import { RouterProvider } from 'react-router-dom';
import { ConfigProvider } from 'antd';
import zhCN_antd from 'antd/locale/zh_CN';
import enUS_antd from 'antd/locale/en_US';
import { router } from './router';
import { rpTheme } from './styles/antdTheme';
import { LanguageProvider } from './i18n';
import { useI18n } from './i18n/context';
import { AuthProvider } from './auth/AuthContext';
import { useSite } from './hooks/useSite';

function AppInner() {
  const { lang } = useI18n();
  // v1.3.0: the browser tab title follows the operator's site name. Done here,
  // once, rather than in each page — the title is a property of the app, and a
  // per-page effect would fight itself on navigation.
  const site = useSite();
  useEffect(() => {
    if (site.site_name) document.title = site.site_name;
  }, [site.site_name]);
  return (
    <ConfigProvider theme={rpTheme} locale={lang === 'zh-CN' ? zhCN_antd : enUS_antd}>
      {/* v0.4.10: AuthProvider wraps the router so every route + the axios
          client can read auth state via useAuth / the unauthorized handler. */}
      <AuthProvider>
        <RouterProvider router={router} />
      </AuthProvider>
    </ConfigProvider>
  );
}

function App() {
  return (
    <LanguageProvider>
      <AppInner />
    </LanguageProvider>
  );
}

export default App;
