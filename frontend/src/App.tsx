import { RouterProvider } from 'react-router-dom';
import { ConfigProvider } from 'antd';
import zhCN_antd from 'antd/locale/zh_CN';
import enUS_antd from 'antd/locale/en_US';
import { router } from './router';
import { rpTheme } from './styles/antdTheme';
import { LanguageProvider } from './i18n';
import { useI18n } from './i18n/context';
import { AuthProvider } from './auth/AuthContext';

function AppInner() {
  const { lang } = useI18n();
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
