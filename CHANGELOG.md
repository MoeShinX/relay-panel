# Changelog

All notable changes to RelayPanel are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

Node-only changes are in **CHANGELOG-NODE.md** (panel and node release on
independent `v*` / `node-v*` tracks since this release).

---

## [Unreleased]

### Added

- **Rule connection controls, storage + API** (no enforcement yet — the node
  half lands separately). Two new per-rule settings, both `0` = off/unlimited so
  an upgrade changes nothing until a rule is explicitly opted in:
  - `max_connections` — cap on concurrent TCP connections, scoped PER NODE.
    Nodes share no state and a group-wide total would need a central allocator
    on the forwarding hot path, so a rule served by 3 nodes admits up to 3x this
    number. The panel ships it to nodes in `ListenerConfig`; a node that doesn't
    understand it ignores it (`#[serde(default)]`).
  - `auto_restart_minutes` — interval for scheduled restarts. A non-zero value
    below `MIN_AUTO_RESTART_MINUTES` (5) is rejected: a shorter loop would drop
    connections faster than clients can reconnect, turning the safety valve into
    the outage.

  Both are edit-only. The atomic create path (`create_rule_with_guard`) doesn't
  carry them, so offering them at create would silently discard the value.
  `PUT /rules/{id}` defaults an omitted one to the rule's CURRENT value rather
  than to 0 — otherwise setting only `max_connections` would silently switch off
  that rule's scheduled restart.

- **`restart_rule` wire message + `node_supports_restart_rule` version gate**
  (1.2.0+) in `relay-shared`. Nothing sends it yet; the panel endpoint and the
  node handler land in follow-up PRs.

### Schema

- SQLite Migration **38**, PG revision **21** (`PG_SCHEMA_VERSION` 20 → 21):
  `forward_rules.max_connections` and `forward_rules.auto_restart_minutes`, both
  `NOT NULL DEFAULT 0`. 0 = unlimited/off. A pre-v1.2 rule must come out
  UNCAPPED — if 0 reached a node as a real cap, upgrading would throttle every
  existing rule to zero connections; `max_connections_zero_means_unlimited_on_the_wire`
  pins that.

---

## [1.1.3] - 2026-07-16

### Fixed

- **systemd-managed nodes are no longer wrongly shown as "手动运行" (manual) in
  node status, and their one-click upgrade button now appears.** The node
  correctly reported its `install_method` ("systemd" | "docker" | "manual"), but
  the panel's status-report handler dropped the field when persisting the node
  status, so the frontend always saw it as unset and resolved every node to the
  "manual" upgrade state — showing "手动运行：不支持一键升级（退出后无人拉起）"
  and hiding the upgrade action on legitimately systemd-managed nodes. The panel
  now persists `install_method`; no node re-install is needed — an already
  running node surfaces the correct state on its next status report.

### Changed

- **The panel Docker image is now multi-arch (`linux/amd64` + `linux/arm64`).**
  ARM64 servers can pull and run the panel image directly. Each architecture is
  compiled natively on its own GitHub-hosted runner (no QEMU / cross-toolchain);
  the two per-arch images are merged into one manifest and the release verifies
  both architectures are present. Node binaries already supported amd64/arm64.

## [1.1.2] - 2026-07-12

### Fixed

- **Auto-assigned listen ports now respect the device group's `port_range`.**
  When a rule was created with the port left on `auto`, the panel ignored the
  inbound group's configured `port_range` entirely and always drew from a
  hardcoded 10000-65535 — so a group set to e.g. `65000-65100` still handed out
  2xxxx ports. Auto-assignment now draws from the group's `port_range`: an
  explicit range is honored verbatim (including sub-10000 ports the admin opted
  into), while the unset/default `1-65535` sentinel maps to the safe 10000-65535
  pool so a never-customized group never auto-assigns a system port. Manual port
  entry, per-group/per-socket-type conflict detection, and the frontend's
  `10000-65535` default display are unchanged.
- **A full port range now returns a clear error instead of "数据库错误".** When
  every port in a group's range is taken, rule creation returns a 400 naming the
  exhausted range (`设备组端口范围 X-Y 已全部占用…`) rather than a generic 500.

## [1.1.1] - 2026-07-08

### Changed — panel & node now release on independent tracks

- **A panel update no longer rebuilds or republishes the node.** Panel releases
  are tagged `vX.Y.Z` and node releases `node-vX.Y.Z`; the two version numbers
  no longer have to match. The `v*` tag builds ONLY the panel image + panel
  GitHub Release; the `node-v*` tag builds ONLY the node binaries + node image
  + node GitHub Release. `relay-panel-node:latest` is untouched by a panel
  release, and vice versa. (`docker-release.yml` is now panel-only; a new
  `node-release.yml` handles the node track; `binary-release.yml` was removed.)
- **The Dockerfile compiles only what each image needs** (`panel-build` /
  `node-build` stages with per-crate `cargo build -p …`), so a panel image
  build no longer compiles `relay-node`.
- **`release-check.sh` takes a `panel` / `node` subcommand:**
  `bash scripts/release-check.sh panel 1.1.1` checks only the panel version
  locations; `… node 1.1.0` checks only the node locations. A panel release no
  longer requires `crates/node` to match, and a node release no longer requires
  the panel to match. A bare version still defaults to panel (backwards
  compatible). `docs/VERSIONS.md` documents the two independent version sets.
- **`docker-compose.release.yaml`** uses independent `RELAYPANEL_PANEL_TAG` /
  `RELAYPANEL_NODE_TAG` overrides so a panel upgrade leaves the node image pin
  unchanged.

### Fixed — node version is no longer measured against the panel version

- **`/system/version`** now returns `latest_node_version` (highest `node-v*`
  tag) and `node_version_check_failed` alongside the panel `latest_version`.
  The node-status UI compares each node's `node_version` against
  `latest_node_version` — NOT the panel version — so a panel-only upgrade (e.g.
  panel 1.2.0 with node still on 1.1.0) no longer makes a current node look
  outdated or offers a non-existent 1.2.0 node upgrade.
- **The directed node-upgrade command** targets `latest_node_version`, not the
  panel's own version. If the node-version lookup fails, the upgrade endpoint
  returns 503 instead of falling back to the panel version (a panel-only
  release can never command a node to download a non-existent asset).
- **Protocol-incompatible nodes** now show "protocol incompatible" in the
  upgrade column too (previously only the status column did), taking priority
  over the version status. **A failed node-version check** shows a neutral
  state instead of a wrong green check or upgrade button. **A node newer than
  the latest node release** is shown as a "leading build" and never downgraded.
  The mobile node list now has the SAME upgrade affordance (version tag +
  upgrade button / docker-hint / manual-disabled / offline-disabled / protocol-
  incompatible ladder) as the desktop table, via a shared `resolveNodeUpgrade`
  helper so the two views can't drift — and it compares against
  `latest_node_version` like the desktop, never the panel version.
- **Node self-upgrade download URLs** use the `node-v{version}` path from 1.1.1
  onward, with a bounded fallback to the legacy `v{version}` path for 1.1.0 and
  earlier (where those binaries were originally published). The
  `relay-node-install.sh` installer queries the latest `node-v*` tag from
  GitHub (never guessing the panel version), supports `--version X.Y.Z`, and
  skips re-download/restart when the installed binary already reports that
  version.

### Fixed — node release gating & installer re-bind

- **`:latest` and the published GitHub Release are now promoted only AFTER
  verification passes.** The node release workflow previously pushed
  `:X.Y.Z` and `:latest` in one build step AND created the GitHub Release as
  stable + `make_latest: true` before `verify` ran — so a release whose image
  reported the wrong version (or whose binary failed sha256) had already
  repointed `:latest`, marked a broken node version as the repo's "Latest"
  (hijacking the README's "latest panel version" badge), and left an advertised
  stable Release behind. Now: `docker-node` pushes the version tag only;
  `build-and-upload` creates the Release as a **draft** (`draft: true`) — GitHub's
  public `/releases` list omits drafts, so a verify-failed node version can
  never leak into the panel's `latest_node_version` (`ALLOW_PRERELEASE_UPDATES`
  includes prereleases, and the installer doesn't filter them, so a prerelease
  would have leaked); `verify` runs (sha256 + binary `--version` + image
  `--version`) and is authenticated so it can still download the draft's assets;
  only then does `promote-latest` re-tag the verified `:X.Y.Z` image as
  `:latest` (`docker buildx imagetools create`) and `publish-release` publish
  the Release (`draft: false`, `prerelease: false`, `make_latest: false`). A
  failed release stays an invisible draft and never moves `:latest` or the repo
  Latest pointer.
- **Re-running the installer at the same version now refreshes the panel binding
  and systemd unit instead of exiting.** Previously an "already at version X"
  detection exited immediately, so re-running with a new `-t`/`-u` (to repoint
  the node at a different panel or rotate its token) silently did nothing. Now
  only the binary download/swap is skipped; the start script (PANEL_URL /
  NODE_TOKEN), the env file, and the systemd unit are rewritten and the service
  is restarted, so the new panel address/token take effect without touching the
  binary.
- **The installer now reports the version it actually installs** (the resolved
  `TARGET_VERSION`, which may come from `--version`), not the script's bundled
  `SCRIPT_VERSION`, in its download/summary/checksum-failure messages.

### Changed — UI, mobile, performance, accessibility (PR4)

- **Mobile node list now shows the version + a one-click upgrade affordance.**
  The mobile card mirrors the desktop upgrade ladder exactly (already-latest →
  green check; systemd+behind+online → upgrade button; docker → "update image";
  manual/unknown → disabled; offline → disabled; protocol-incompatible → red
  tag), via a shared `resolveNodeUpgrade` helper so the two views can't drift.
  Non-admins see no upgrade UI.
- **Pages are now code-split.** Every page (`Dashboard`/`Rules`/`Users`/`Plans`/
  `Groups`/`NodeStatus`/…) loads via `React.lazy` on first navigation, so the
  login page no longer pulls in the admin pages. Vendor libs are split into
  their own chunks (`react-vendor`, `antd`, `icons`, `semver`). The login entry
  chunk is ~115 KB (was the whole app); the heavy antd chunk is isolated and
  caches independently.
- **Cleaned up real Ant Design v6 deprecation warnings** (verified by running
  the test suite first): `Drawer width` → `size`, `Alert message` → `title`,
  `Space direction` → `orientation`. Also silenced the known jsdom
  `getComputedStyle(pseudoElt)` "Not implemented" noise in the test setup by
  dropping the pseudo-element arg (a targeted fix — real console warnings are
  still surfaced).
- **Accessibility.** Icon-only buttons (rule target move-up / move-down / delete,
  node upgrade, install-command copy) now have `aria-label`s. Login and Register
  inputs carry an `aria-label` instead of relying on `placeholder`. Async result
  regions (import results, diagnose loading) use `aria-live="polite"` /
  `aria-busy`. Mobile upgrade tap targets are ≥32×32 px.

### Changed

- **The minimal share-export now has a regression test pinning its round-trip.**
  The export format (`[{"dest":["host:port"],"listen_port":10000,"name":"…"}]`,
  enabled targets only, IPv6 bracketed) and the import validation previously
  lived as private functions inside `Rules.tsx`, so a future change could have
  silently broken the "export pastes straight back into import" property. They
  are extracted into a pure `frontend/src/utils/rulesIO.ts` module
  (`buildExportJSON`, `validateImportEntry`, `parseDest`, `ruleTargets`) and
  covered by `rulesIO.test.ts`, which asserts that a rule exported by
  `buildExportJSON` always re-imports cleanly (every entry passes
  `validateImportEntry`, and the parsed targets match the original enabled
  targets) for single/multi target, IPv4/IPv6, disabled-target filtering, and
  whitespace-trim cases. `Rules.tsx` now imports the shared helpers (removing
  the duplicated dest regex).

### Fixed

- **Creating a forward rule no longer cross-writes into a different rule when
  two inbound groups reuse the same listen port.** Previously, after the rule
  row was inserted, the new rule's id was recovered by re-querying
  `(owner_uid, listen_port)` — which ignored `device_group_in`. Because the
  port-uniqueness constraint is *per inbound group*, two rules on two groups
  can legally share a port, and the lookup returned the wrong (first) rule,
  so its targets, load-balance strategy and rate limits were overwritten. Rule
  creation now does the row INSERT + targets + load-balance strategy + rate
  limits + tunnel profile in a **single transaction** and takes the new id
  directly from the INSERT (SQLite `last_insert_rowid()` / PostgreSQL
  `RETURNING id`), so any mid-creation failure rolls back completely (no
  half-rule) and the side-tables always land on the right row. Existing
  port-conflict, `max_rules` quota and ownership checks are unchanged.
  (`create_rule_full` on the Repository trait, used by `create_rule`.)
- **Every password input now enforces the backend's 8–72 UTF-8-byte rule.**
  Previously MainLayout / Account change-password and the admin create-user form
  used an antd `min: 6` *character* rule (UTF-16 code units, no upper bound),
  while Register / ForcePasswordChange / admin-reset used a copy-pasted
  TextEncoder byte check — so a 6-char password could be set via change-password
  but never re-set via self-service, and a >72-byte password passed the client
  only to be rejected by bcrypt. All six inputs now share one
  `validatePassword` util (`frontend/src/utils/password.ts`) that counts UTF-8
  bytes via `TextEncoder` (exactly matching `password.len()` in Rust), and the
  zh/en hint text is unified to "8–72 bytes (UTF-8)".
- **`validateImportEntry` now runtime-type-checks every field** of the pasted
  JSON (it receives `unknown`, straight from `JSON.parse`). A malformed paste —
  e.g. `{"name": 123, "listen_port": "80", "dest": "1.2.3.4:80"}`, a bare
  primitive, `null`, or an array where an entry object was expected — now
  produces a clean per-entry "❌" error in the import results instead of
  throwing (`.trim is not a function`, etc.). `handleImport` likewise labels
  non-object entries safely and only casts via the new `asValidatedEntry`
  helper after validation. Covered by 9 new "anomalous input does not crash"
  tests.

### Security

- **Security response headers are now set on every panel response** (API + the
  static SPA): `X-Content-Type-Options: nosniff`,
  `Referrer-Policy: strict-origin-when-cross-origin`, `X-Frame-Options: DENY`,
  a strict `Content-Security-Policy` (`default-src 'self'`, `script-src 'self'`,
  `object-src 'none'`, `base-uri 'self'`, `frame-ancestors 'none'`,
  `form-action 'self'`), and a conservative `Permissions-Policy` (camera,
  microphone, geolocation, USB, etc. disabled). `style-src` is widened to
  `'self' 'unsafe-inline'` because Ant Design v6 injects runtime CSS-in-JS;
  `script-src` stays strict (Vite's production build has no inline scripts).
  HSTS is intentionally NOT set by the panel — it belongs to the HTTPS / reverse
  proxy layer (Caddy). Each header is `if_not_present`, so a stricter header set
  by an edge proxy is preserved.
- Pinned by regression test: a freshly-registered user has **no usable device
  groups** by design (`all_device_groups = false`, `user_device_groups` empty),
  so they cannot forward until a plan or admin grants authorization. Covered
  on both SQLite and PostgreSQL to guard against a future auto-grant-on-register
  change flipping this silently.
---

## [1.1.0] - 2026-07-02

Minor release headlined by **one-click remote node upgrades** from the panel,
capping off the plan-model / performance / correctness work of the 1.0.x line.

### Added

- **One-click node upgrade.** The Node Status page shows a per-node upgrade
  action (active when a node is behind the panel version). Clicking it directs
  that node to self-update: it downloads the panel's exact version from the
  official GitHub release for its architecture, verifies the published sha256,
  backs up its current binary, atomically swaps, and restarts (systemd). Safety:
  - The command carries no URL/binary — the node only pulls the official release
    and verifies the hash, so it can never be made to run arbitrary code.
  - **Upgrade-only:** the target must be a valid semver strictly newer than the
    running version, so a compromised panel can't force a downgrade to an old,
    vulnerable build.
  - **Install-aware:** only systemd nodes self-upgrade; docker nodes show
    "update the image", and manual runs are disabled (nothing would restart
    them). Nodes report their install method for this.
  - Single-flight + mandatory backup, so repeated clicks can't corrupt the
    binary and a failed backup aborts the swap.
- Node binaries continue to ship for both **amd64 and arm64** (static musl).

### Fixed

- The default "free" plan no longer reappears in the shop after every panel
  update. It is now seeded only on a fresh (empty) database, so an admin who
  deletes it (once other plans exist) won't see it come back on restart.
- Shop plan cards no longer render ragged when a plan grants no lines — the
  "granted lines" row now shows "无 / None" so all cards stay aligned.

---

## [1.0.9] - 2026-07-02

Finalizes the plan model to a **single current plan** (renew vs. switch), a
substantial **UDP/TCP forwarding performance pass**, and a round of correctness
fixes across billing, admin actions, and the rule editor.

### Changed

- **A user holds exactly one current plan.** Buying the **same** plan *renews*
  it (traffic stacks; a time plan's expiry extends from its current end). Buying
  a **different** plan *switches*: `traffic_limit` becomes the new plan's quota
  (not stacked), `traffic_used` resets to 0, the expiry is recomputed from now,
  and device-group authorization is fully replaced. The shop and the admin panel
  both confirm before a switch. This replaces the short-lived additive model —
  to give a user several lines, sell a bundled plan.
- **Rate-limited rules pick up limit changes without a node restart.** A rule's
  upload/download cap is part of the listener fingerprint now, so changing or
  clearing a limit hot-reloads the listener instead of running the old cap until
  the next restart.

### Added

- Shop plan cards resolve the **names** of the lines a plan grants server-side
  (previously they could show a raw `#id` for lines the buyer wasn't yet
  authorized for).
- **DNS cache** for outbound TCP targets: domain targets no longer re-resolve on
  every new connection, with a stale-entry fallback when the resolver blips.

### Performance

- **UDP forwarding.** Removed the per-packet full-table session scan; made the
  traffic counter lock-free (atomic per rule); moved the outbound bind/connect
  out of the session lock; sharded both the per-listener session map and the
  connection tracker (concurrent maps); and enlarged UDP socket buffers. Large
  reduction in per-packet lock contention on high-PPS links.

### Fixed

- **Traffic billing** is charged on upload **and** download (their sum × the
  line's rate); this is now documented explicitly.
- Plan **create** and admin **remove-plan** run as single transactions, so a
  mid-operation DB error can't leave a plan with no lines or a half-revoked user.
- **Batch rule delete** reports actual success/failure counts instead of always
  claiming every selected rule was deleted.
- List endpoints (plans / shop) return a real error on a DB failure instead of a
  fake empty "success" list.
- `update_plan` rejects setting `duration_days = 0` on a time plan.
- Editing only a Basic-tab field of a rule (e.g. the listen port) no longer
  wrongly demands "add a forward target".
- `relay-node-install.sh` no longer fails with a `getcwd` error when run from a
  directory that has since been deleted.
- The device-group edit form no longer offers the unused **outbound/egress**
  type; the inbound-group dropdown drops the redundant "(shared)" suffix; the
  rule list shows all target IPs on hover.

---

## [1.0.8] - 2026-07-01

A performance & correctness release for the node's TCP forwarding path
(latency/jitter fixes plus zero-copy for unlimited rules), a switch to
**replace-semantics** for plan-linked device-group authorization, and a small
round of admin UI polish.

### Added

- **Zero-copy TCP forwarding (Linux).** Unlimited rules now forward with
  `splice(2)` (kernel pipe, no userspace copy), cutting CPU and latency on long
  forwarding chains. Rate-limited rules keep the userspace copy path so the
  token bucket still applies; byte counters stay accurate on both paths.

### Changed

- **Plan authorization now replaces instead of only expanding.** Buying a plan
  sets the user's device-group authorization to exactly what the plan grants
  (a per-group plan resets `all_device_groups`; an all-groups plan clears any
  stale per-group rows). This supersedes the v1.0.7 "append-only / only ever
  expands" behavior, which could leave a downgraded user over-authorized.
- **Auto-paused rules resume symmetrically.** A new `auto_paused` flag marks
  rules the *system* paused (plan removal / expiry) versus ones a human paused;
  only the former auto-resume when authorization is restored, so a manual pause
  is never silently undone.
- **Larger forwarding buffer, smarter pacing.** The userspace copy buffer moved
  to 32 KiB and `TCP_NODELAY` is now set on every TCP socket (both accepted and
  dialed) to remove Nagle/delayed-ACK stalls that compounded across hops.
- **Admin UI.** The edit-user modal no longer exposes raw device-group toggles
  (authorization is driven by the plan); the plan expiry is editable only for
  time-based plans (grayed out for data plans); the delete-plan button is
  enabled only when a plan is selected.

### Fixed

- **Rate limiter head-of-line blocking & stall.** The limiter no longer holds
  its lock across the pacing sleep (one slow rule could stall others), and a
  chunk larger than the burst capacity no longer loops forever (debt-based
  tokens). This is the root cause of the reported forwarding jitter.

### Disabled

- **WS / TLS forwarding transports are no longer served.** The frontend already
  hides them; the listener code is kept in-tree but skipped at runtime. TCP and
  UDP are unaffected. (No config migration needed.)

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
