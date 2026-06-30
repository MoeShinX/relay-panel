# Changelog

All notable changes to RelayPanel are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

---

## [1.0.7] - 2026-06-30

A feature release: a self-service **plan shop with billing**, a rewritten
**per-user device-group authorization** model, admin plan management, and a
round of rule/node UI polish.

### Added

- **Plan shop & billing.** Self-service plan purchase (`/shop`) with order
  history and account balance; admin plan CRUD (`/plans`). Buying a plan is an
  atomic balance charge.
- **User suspension.** A suspended user can still log in and buy a plan
  (buying does not auto-unsuspend), but forwarding is gated off.
- **Plan-linked device groups.** A plan can grant device-group access;
  purchasing auto-grants the authorization (append-only — it never silently
  removes access).
- **Device-group rate billing.** Each group has a multiplier (0.1–100); users
  are charged `real bytes × rate` while rule/user byte counters stay real.
- **Admin "edit user plan" panel**, embedded in the edit-user modal: assign an
  existing plan (charges the user's balance), change or remove the plan, and
  edit the expiry. Removing a plan also revokes the user's device-group
  authorization and auto-pauses (but does **not** delete) their rules.
- **Batch pause / resume** on the rules page.
- **Hidden device groups.** A per-group `hidden` toggle hides a group from
  regular users' Node Status page only — rules keep working (still selectable
  for new rules; existing rules forward and display normally). Admins are
  unaffected.

### Changed

- **Per-user device-group authorization replaces user permission groups.** A
  user is either unrestricted (`all_device_groups`) or limited to an explicit
  set of authorized groups; authorization only ever expands.
- **Removed the regular-user dashboard.** Its rules/traffic stats duplicated
  the 个人中心 (Account) page and its line/node counts duplicated Node Status;
  regular users now land on `/account`.
- **Rule form UX.** "TCP + UDP" is now first in the protocol list and the
  default for new rules; data-type plans hide the duration field; the two
  rate-limit inputs are labeled 上行/下行 with a tooltip explaining the
  shared-per-rule / enforced-per-node mechanism.
- **Node Status table** widened the IP column so IPv6 no longer misaligns the
  other columns; status/CPU columns compacted.
- **Rule export is now compact single-line JSON** (`[{…},{…}]`) matching the
  import box; the per-row export button was removed.

### Fixed

- **Deleting a plan no longer leaves residual device-group access.** Because
  authorization "only ever expands", a removed plan now also clears
  `all_device_groups` + `user_device_groups` and pauses the affected rules.
- **Resume-rule authorization bypass.** A restricted user could un-pause a rule
  on a device group they were not authorized for; `update_rule` now re-checks
  authorization on resume.
- **Regular user's rule edit** showed "未配置" for a shared group's connect
  host; it now resolves from the merged shared-group info.
- **Batch delete, admin rule isolation, and user-group UX** fixes.

---

## [1.0.6] - 2026-06-29

### Fixed

- **Rule export always returns a JSON array.** Single-rule exports previously
  emitted a bare object `{…}` instead of a one-element array `[{…}]`, making
  the exported JSON incompatible with the import box (which expects the array
  form `[{"dest":[…],"listen_port":…,"name":"…"}]`). Export now always wraps
  the result in an array, so copy-paste round-trips work regardless of the
  number of rules selected.
- **Imported rules were attributed to the admin instead of the target user.**
  When an admin opened a user's rule list via `/rules?owner_uid=X` and used
  the bulk-import feature, the created rules were owned by the admin account.
  The `owner_uid` parameter is now forwarded in the import POST request,
  matching the behaviour of the manual "add rule" form.

---

## [1.0.5] - 2026-06-29

### Fixed

- **Device-group node list crashed the page.** Expanding a device group threw
  `K.slice is not a function` and blanked the screen. The node-list ID column
  had no `dataIndex`, so antd handed the whole row object to `render()` instead
  of the `node_id` string. Now bound to `dataIndex: "node_id"`.
- **Default user-group remark mojibake.** The seeded default group's remark
  rendered as `Default group â?? all device groups allowed` on PostgreSQL
  connections whose `client_encoding` wasn't UTF-8, because the seed used an
  em dash (U+2014). Replaced with an ASCII hyphen across all four seeds (SQLite
  + PG, schema + migration); SQLite Migration 31 / PG revision 14 normalizes the
  remark on existing databases.
- **PG migration for the remark fix never ran.** `PG_SCHEMA_VERSION` was still
  13, so the early `current >= PG_SCHEMA_VERSION` guard skipped the new
  revision-14 UPDATE. Bumped to 14 so the migration executes and the baseline
  seed assertion passes.
- **TCP egress failures were undiagnosable on multi-NIC nodes.** `handle_tcp_connection`
  collapsed every per-target failure into a flat "no target available",
  discarding the real cause. Each attempt now preserves its classified outbound
  error (DNS / timeout / connection refused / source-bind), and the final
  log/error joins all per-target reasons.

### Changed

- **Node installer surfaces the dual-stack / egress env vars.** The generated
  `relay-node.env` now carries commented examples for `LISTEN_IPV4` /
  `LISTEN_IPV6` and `OUTBOUND_INTERFACE` / `OUTBOUND_BIND_IPV4` (illustrative
  IPs only, never defaults), so multi-NIC operators can discover them at install
  time. Defaults unchanged: dual-stack listen, system-routed egress, no source
  bind.

---

## [1.0.4] - 2026-06-26

### Fixed

- **Atomic group update + pause.** `update_user_group_with_pause` runs
  group update and rule re-evaluation in a single transaction. On pause
  failure, the group update is rolled back so the authorization state is
  NOT partially changed. Previously, a pause failure returned 500 but left
  the authorization change already written, causing some rules to continue
  forwarding with elevated access.

## [1.0.3] - 2026-06-26

### Fixed

- **Node-side traffic counter poison-pill.** When a rule was deleted, stale
  bytes in the node's `TrafficCounter` were never pruned. The next report batch
  was rejected atomically, the node kept retrying the same bytes, and traffic
  billing froze until node restart. The counter entry is now pruned when its
  rule disappears from the config and no live listener still references it.
- **Per-rule export button had no label.** The icon-only export button in the
  rules action column now shows 导出 / Export, matching its siblings.

### Changed

- **New 石墨靛蓝 / Graphite + Indigo UI theme.** Graphite sidebar, indigo accent,
  larger radii, hairline borders, flatter buttons — replacing the default
  deep-blue admin-template look. antd v6 token-driven; no business components
  touched.
- **Self-hosted Noto Sans SC (思源黑体)** as the UI font, for crisp and
  consistent CJK rendering across platforms.
- **Forced password-change notice reworded** (zh + en) to cover both the
  admin-reset and create-with-must-change cases, instead of only "an admin
  reset your password".

---

## [1.0.2] - 2026-06-26

### Fixed

- **PostgreSQL: creating a forward rule failed with `database error`.** The
  owner-scope ownership guard in `replace_rule_targets` decoded a `SELECT 1`
  literal as `i64`. PostgreSQL types integer literals as `INT4`, so sqlx
  rejected the `INT8`/`INT4` mismatch. SQLite's dynamic typing masked the bug,
  so it only affected PostgreSQL deployments. Now decoded as `i32`.

---

## [1.0.1] - 2026-06-25

First public release of RelayPanel.

### Highlights

- **TCP/UDP forwarding panel** with relay-node architecture, WebSocket
  real-time config push, and HTTP polling fallback.
- **Multi-plan registration.** Administrators configure which plans are
  available for registration; users pick a plan when signing up.
- **Per-target circuit breaker.** 3 consecutive connect failures → 30-second
  circuit break; all-down fails open (probe mode). Applies to failover and
  round-robin strategies over TCP/WS/TLS.
- **User rule management.** Administrators manage a user's rules directly from
  the user management page; ownership determined by entry point.
- **GeoIP node region display** with built-in primary (ipinfo.io) and fallback
  (ipwho.is) sources. GeoIP cache auto-cleaned on node deletion.
- **SQLite + PostgreSQL dual backend** with compile-time trait enforcement and
  CI-guarded test parity.
- **Dashboard** with node aggregation, traffic statistics, and quota management.
