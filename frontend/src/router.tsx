import { createBrowserRouter } from 'react-router-dom';
import MainLayout from './layouts/MainLayout';
import Login from './pages/Login';
import Register from './pages/Register';
import Rules from './pages/Rules';
import Groups from './pages/Groups';
import Users from './pages/Users';
import NodeStatus from './pages/NodeStatus';
import Account from './pages/Account';
import SystemSettings from './pages/SystemSettings';
import Plans from './pages/Plans';
import Shop from './pages/Shop';
import ForcePasswordChange from './pages/ForcePasswordChange';
import Forbidden from './pages/Forbidden';
import RoleHome from './RoleHome';
import { RequireAuth } from './RequireAuth';
import { RequireAdmin } from './RequireAdmin';

export const router = createBrowserRouter([
  { path: '/login', element: <Login /> },
  // v0.4.10 PR3: public self-service registration (guarded by registration-status on mount).
  { path: '/register', element: <Register /> },
  // v0.4.10 PR4: forced password change. TOP-LEVEL (not under RequireAuth) so
  // RequireAuth's must-change redirect can't loop back to itself. The page
  // itself relies on the logged-in token to call PUT /user/password.
  { path: '/force-password-change', element: <ForcePasswordChange /> },
  {
    path: '/',
    element: <RequireAuth><MainLayout /></RequireAuth>,
    children: [
      // v0.4.10: the index route renders RoleHome, which switches between
      // Dashboard (admin) and UserDashboard (regular) based on the
      // server-verified role. No RequireAdmin — regular users land here.
      { index: true, element: <RoleHome /> },
      // Owner-scoped resources — any authenticated user manages their own.
      { path: 'rules', element: <Rules /> },
	      { path: 'groups', element: <Groups /> },
      { path: 'nodes', element: <NodeStatus /> },
      { path: 'node-status', element: <NodeStatus /> },
      // v1.0.8: self-service shop (plan purchase + order history).
      { path: 'shop', element: <Shop /> },
      // v1.0.8: admin plan management (CRUD).
      { path: 'plans', element: <RequireAdmin><Plans /></RequireAdmin> },
      // v0.4.20: tunnel-profiles route hidden; component kept for future recovery.
      { path: 'users', element: <RequireAdmin><Users /></RequireAdmin> },
      { path: 'settings', element: <RequireAdmin><SystemSettings /></RequireAdmin> },
      // Account is open to every authenticated user (admin or not).
      { path: 'account', element: <Account /> },
      // v0.4.10: explicit 403 page for admin-only routes a regular user
      // navigates to directly (vs the old silent redirect to /account).
      { path: '403', element: <Forbidden /> },
    ],
  },
]);
