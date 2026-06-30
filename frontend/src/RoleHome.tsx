import { Spin } from 'antd';
import { Navigate } from 'react-router-dom';
import { useAuth } from './auth/useAuth';
import Dashboard from './pages/Dashboard';

/**
 * v0.4.10: the index-route switch. Renders the admin Dashboard for admins.
 * v1.0.7: the regular-user dashboard was removed — its stats (rules / traffic)
 * duplicated the 个人中心 (Account) page, and its line/node counts duplicated
 * Node Status. Regular users are redirected to /account instead. Kept in its
 * own module (not in router.tsx) so router.tsx only exports route config —
 * this satisfies react-refresh/only-export-components.
 *
 * Shows a spinner until authReady flips, so a page refresh doesn't redirect or
 * flash the wrong home while /user/me resolves the real role.
 */
export default function RoleHome() {
  const { isAdmin, authReady } = useAuth();
  if (!authReady) {
    return (
      <div style={{ display: 'flex', justifyContent: 'center', padding: 48 }}>
        <Spin />
      </div>
    );
  }
  return isAdmin ? <Dashboard /> : <Navigate to="/account" replace />;
}
