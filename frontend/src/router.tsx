// router.tsx is the route configuration (not a React component file). The
// react-refresh/only-export-components rule flags the `lazy(() => ...)`` page
// imports because it can't tell they're components — but fast-refresh doesn't
// apply to a router config, so suppress the rule for this file only (targeted,
// not a blanket suppression of real warnings).
/* eslint-disable react-refresh/only-export-components */
import { lazy, Suspense } from 'react';
import { createBrowserRouter } from 'react-router-dom';
import { Spin } from 'antd';
// EAGER: the authenticated shell + its route guards load up front so the
// sidebar/menu renders immediately after login (and the guards run before any
// lazy chunk is fetched, so a 403 redirect doesn't wait on a page download).
import MainLayout from './layouts/MainLayout';
import { RequireAuth } from './RequireAuth';
import { RequireAdmin } from './RequireAdmin';
// LAZY: every full page is code-split so the login page (and the initial JS
// payload) doesn't pull in Dashboard / Rules / Users / Plans / etc. Each page
// loads on first navigation. The authenticated pages share a <Suspense> in
// MainLayout (wrapping <Outlet/>); the top-level public pages wrap their own.
const Login = lazy(() => import('./pages/Login'));
const Register = lazy(() => import('./pages/Register'));
const ForcePasswordChange = lazy(() => import('./pages/ForcePasswordChange'));
const Rules = lazy(() => import('./pages/Rules'));
const Groups = lazy(() => import('./pages/Groups'));
const Users = lazy(() => import('./pages/Users'));
const NodeStatus = lazy(() => import('./pages/NodeStatus'));
const Account = lazy(() => import('./pages/Account'));
const SystemSettings = lazy(() => import('./pages/SystemSettings'));
const Plans = lazy(() => import('./pages/Plans'));
const RedeemCodes = lazy(() => import('./pages/RedeemCodes'));
const Shop = lazy(() => import('./pages/Shop'));
const Forbidden = lazy(() => import('./pages/Forbidden'));
const RoleHome = lazy(() => import('./RoleHome'));

/** A centered spinner used as the Suspense fallback for lazy pages. */
function PageSpin() {
  return (
    <div style={{ textAlign: 'center', padding: 48 }}>
      <Spin />
    </div>
  );
}

/** Wrap a lazy element in its own Suspense (for top-level public routes that
 *  don't go through MainLayout's <Outlet/> Suspense). */
function withSuspense(el: React.ReactElement) {
  return <Suspense fallback={<PageSpin />}>{el}</Suspense>;
}

export const router = createBrowserRouter([
  { path: '/login', element: withSuspense(<Login />) },
  // v0.4.10 PR3: public self-service registration (guarded by registration-status on mount).
  { path: '/register', element: withSuspense(<Register />) },
  // v0.4.10 PR4: forced password change. TOP-LEVEL (not under RequireAuth) so
  // RequireAuth's must-change redirect can't loop back to itself. The page
  // itself relies on the logged-in token to call PUT /user/password.
  { path: '/force-password-change', element: withSuspense(<ForcePasswordChange />) },
  {
    path: '/',
    element: <RequireAuth><MainLayout /></RequireAuth>,
    children: [
      // v0.4.10: the index route renders RoleHome. Admins get the Dashboard;
      // regular users are redirected to /account (the regular-user dashboard
      // was removed in v1.0.7). No RequireAdmin — regular users land here.
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
      // v1.2.0: redeem-code management (admin-only).
      { path: 'redeem-codes', element: <RequireAdmin><RedeemCodes /></RequireAdmin> },
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
