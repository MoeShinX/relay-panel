# Notification Settings Operations Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Provide structured notification controls, persistent delivery history, selectable node events, and opt-in repeated offline alerts.

**Architecture:** Keep notification configuration in the existing `notify:config` KVS JSON. Persist the latest per-channel delivery outcomes under a separate KVS prefix, expose them through an admin-only API, and let the node watcher use pure state decisions for one-shot, repeat, recovery, and version-outdated alerts. The React page renders the API state as rule, channel, and history sections.

**Tech Stack:** Rust/Axum/Serde/Chrono/Semver, existing repository KVS abstraction, React 19, TypeScript, Ant Design, Vitest.

---

### Task 1: Add notification configuration and delivery-history domain tests

**Files:**
- Modify: `crates/panel/src/service/notify.rs`
- Modify: `crates/panel/src/service/node_watch.rs`

- [ ] **Step 1: Write failing Rust tests** for: missing `notify_offline` defaults to true; a history list is newest first and capped at 100; an outage repeat occurs only after its configured interval; a lower semver is outdated while an unparsable version is ignored.
- [ ] **Step 2: Run** `cargo test -p relay-panel notify node_watch` and confirm the new tests fail because the config fields, history helpers, repeat state, and version decision do not exist.
- [ ] **Step 3: Implement minimal domain types and pure decisions**: `NotifyConfig` fields, `DeliveryLogEntry`, KVS history helpers, `NodeState` alert timestamp, and semver version comparison.
- [ ] **Step 4: Run** `cargo test -p relay-panel notify node_watch` and confirm all targeted tests pass.

### Task 2: Wire history and selectable events through the API and watcher

**Files:**
- Modify: `crates/panel/src/api/notify.rs`
- Modify: `crates/panel/src/api/mod.rs`
- Modify: `crates/panel/src/service/node_watch.rs`

- [ ] **Step 1: Write failing API/watcher tests** proving the public configuration includes the three new fields and that successful and failed per-channel outcomes are written to history.
- [ ] **Step 2: Run** `cargo test -p relay-panel api::notify node_watch` and confirm failure because the history endpoint and recording calls do not exist.
- [ ] **Step 3: Implement** `GET /admin/settings/notify/history`, update request/public projections and validation, record test-send outcomes, record watcher outcomes, and issue one version-outdated alert per node/latest-version pair.
- [ ] **Step 4: Run** `cargo test -p relay-panel api::notify node_watch` and confirm the targeted tests pass.

### Task 3: Build the notification settings UI and tests

**Files:**
- Create: `frontend/src/pages/NotifySettings.test.tsx`
- Modify: `frontend/src/pages/NotifySettings.tsx`
- Modify: `frontend/src/api/types.ts`
- Modify: `frontend/src/i18n/zh-CN.ts`
- Modify: `frontend/src/i18n/en-US.ts`

- [ ] **Step 1: Write failing Vitest cases** proving the page renders the three labelled sections, reports each channel’s state, and shows an empty history state plus a failure detail row.
- [ ] **Step 2: Run** `npm test -- NotifySettings.test.tsx` from `frontend` and confirm the tests fail because no history endpoint or status labels are rendered.
- [ ] **Step 3: Implement** API types, translations, status tags, labelled channel switches, event toggles, repeat interval field, history table/empty state, and history reload after test send.
- [ ] **Step 4: Run** `npm test -- NotifySettings.test.tsx` and confirm the tests pass.

### Task 4: Verify, preview, and hand off

**Files:**
- Verify only.

- [ ] **Step 1: Run** `cargo fmt --check`, `cargo clippy -p relay-panel -- -D warnings`, and `cargo test -p relay-panel`.
- [ ] **Step 2: Run** `npm run typecheck`, `npm run lint`, `npm test`, and `npm run build` from `frontend`.
- [ ] **Step 3: Start the local stack**, open the notification settings page in the local browser, and inspect desktop and narrow viewport rendering.
- [ ] **Step 4: Commit the implementation** with `feat(notify): add delivery history and alert controls`, then create a separate pull request.
