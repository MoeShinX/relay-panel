# Changelog

All notable changes to RelayPanel are documented here.
The format is based on [Keep a Changelog](https://keepachangelog.com/).

Node-only changes are in **CHANGELOG-NODE.md** (panel and node release on
independent `v*` / `node-v*` tracks since this release).

---

## [Unreleased]

### Changed

- **The panel fits a phone.** Below 768px wide the sider is gone — even
  collapsed it kept an 80px icon strip, a fifth of a phone screen — and the
  menu slides in from a button in the header. The header keeps the
  announcements bell and folds language, password, GitHub and logout into one
  menu; before, it ran off the screen. Page titles no longer shrink to one
  character per line beside their buttons (the buttons wrap below), card
  headers wrap their controls under the title, the dashboard's three stat
  cards stay in one row, and every table scrolls inside itself instead of
  widening the whole page. The login, registration and forced
  password-change cards, and the rule dialog's target address, fit a narrow
  screen. Tablets and desktops are unchanged.

## [1.2.12] - 2026-10-04

Ships alongside `node-v1.2.6`; neither requires the other, and the config
protocol is unchanged at version 4. One schema change, applied on start: a
table of traffic batch ids (SQLite migration 45, PostgreSQL revision 28).

Upgrading: "Update now" on the dashboard, or `git pull --quiet && ./deploy.sh`.
Three things to know first:

- **`JWT_SECRET` must now be at least 32 characters.** `deploy.sh` — which the
  one-click update runs — replaces a shorter one, which signs everyone out
  once. If you run the compose file by hand, set a new one before upgrading or
  the panel will not start.
- **The panel no longer runs as root in its container.** The existing database
  is handed over on the first start, with nothing to do — unless you bind-mount
  `/app/data` (it becomes owned by uid 10001) or run the CLI with
  `docker compose exec` (add `-u relaypanel`).
- **Logins are now also limited per client IP** (20 a minute). A reverse proxy
  that reaches the panel from a public address — one on another server —
  counts as a single client for everyone behind it.

A re-sent traffic report is billed once only when the node runs `node-v1.2.6`
as well.

### Added

- **The node-status page shows when a node has no control channel.** A node
  can be online — reporting status over HTTP every cycle — while its WebSocket
  control channel is down. It keeps forwarding, but config changes wait for the
  next poll and neither a remote upgrade nor a diagnosis can reach it; until now
  that was invisible, and surfaced only as an error after clicking "Upgrade".
  Such nodes now carry a "Polling only" tag explaining what that means and how
  to find the cause on the node, and their upgrade action is disabled with the
  reason instead of failing on click.

- **Rules can be exported one inbound group at a time, and the dashboard
  counts each group's rules.** "Export all" put every machine's targets into
  one file. With a group filter (or a search) on the rules page, the export
  menu now also offers "export filtered (N)", the file named after the group;
  import already asks which group to load into, so this is how a new machine
  gets an old one's rules. The dashboard's node table has a rules column —
  enabled / total per group — to check that a new machine lines up with the
  old ones.

### Changed

- **A manual upgrade backs up the SQLite database too.** Only the one-click
  update used to copy the database to `backups/` before the new version (and
  its schema migrations) started; `git pull && ./deploy.sh` did not. Both now
  use the same backup, and it is stricter than the one-click update's was:
  - The copy is taken only once the panel is confirmed stopped. If it cannot
    be stopped (or its state cannot be read), there is no backup and no
    upgrade — a copy taken while the panel writes may be inconsistent, and the
    old one-click update carried on with it regardless.
  - A lookup that fails (listing the containers, reading the data mount or
    volume) stops the upgrade too, instead of counting as "no database". Run
    `deploy.sh` as root: without root the database volume cannot be read and
    the upgrade stops with a message (back the database up yourself and
    re-run with `RELAYPANEL_BACKUP_DONE=1` to go ahead anyway).
  - A panel that was already stopped is backed up too (and left stopped); it
    used to be treated as "no database" and skipped. After `docker compose
    down` the database is found in the leftover volume.
  - The file backed up is the one `DATABASE_URL` names — read from the panel
    container, else from the environment, `.env` or the default — not always
    `data.db`; any other name used to be skipped as "nothing to back up". A
    database outside the data volume (`/app/data`) stops the upgrade with a
    message instead.
  - The newest five are kept. If the backup fails, nothing is changed and the
    old version stays in place; if the new version then fails to start, the
    old container is started again.

  When building from source, the image is built before the panel is stopped,
  so the downtime does not include the compile. PostgreSQL is still not
  backed up automatically.

- **The top bar's GitHub icon sits before "Log out"**, so the order is
  change password → GitHub → log out, with log out last.

### Fixed

- **Concurrent writes no longer fail with a 500 on SQLite.** A purchase,
  redeem or traffic report that ran at the same moment as another write could
  fail outright with "database is busy": its transaction read first and only
  asked for the write lock later, and SQLite refuses that when another write
  has landed in between. Write transactions now take the lock at the start, so
  concurrent requests simply wait their turn. Nothing was ever double-charged
  or lost — nodes retry a failed traffic report — but the request failed.

- **Expiry times are checked and stored in one exact format.** Plan,
  redeem-code and announcement expiry are compared as text, so any spelling
  other than `YYYY-MM-DD HH:MM:SS` (UTC, zero-padded) sorts wrong. The admin
  "adjust plan expiry" API stored whatever string it was given: `2026/10/01`
  or `never` meant the plan never expired, and `2026-10-01T00:00:00Z` lasted
  until the end of that day. Redeem codes and announcements did check the
  format, but kept an unpadded `2026-9-1 00:00:00` as typed — which sorts
  after `2026-09-30`, so it stayed valid for the rest of the month. All three
  now accept only that layout and store it zero-padded. The panel's own date
  pickers always sent the right format.

- **Changing a user's line authorization is all-or-nothing.** The flag, the
  explicit lines and pausing the rules outside them were four separate writes;
  a failure part-way could leave a revoked line's rules running. They are now
  one transaction. (The admin UI no longer edits these directly — plans do —
  but the API still accepts them.)

- **Deleting a line or plan that is still in use no longer says "deleted".**
  The panel refused the deletion, but the page announced success anyway. The
  same applied to deleting a rule or a node status entry that the panel
  rejected. Several admin actions also failed silently on a network error;
  they now say so.

- **Copying new redeem codes works on a plain-HTTP panel.** It used the
  browser clipboard API directly, which only exists on HTTPS.

- **The device-groups page is admin-only in the browser too.** The API already
  refused regular users; opening `/groups` directly now shows the 403 page
  instead of a page of failing requests.

- **`CORS_ORIGINS=*` stops the panel with an explanation** instead of a bare
  crash. Wildcard CORS is deliberately unsupported; list the exact origins.

- **A rule edit is saved completely or not at all.** Its fields were written
  one after another, and some checks ran only after the first writes: an edit
  that changed the name and set a 1-minute restart interval was refused, yet
  the new name was kept. Every check now runs first and the whole edit is one
  transaction.

- **Setting one direction's speed limit no longer removes the other.** An edit
  that sent only the upload limit wrote the download limit as 0 (no limit).
  An omitted direction now keeps its limit; send 0 to remove one. The panel's
  own form always sent both, so this affected API clients.

- **Deleting a device group works on PostgreSQL.** The "is this group still
  used by a rule?" check queried a `fallback_group` column that the rules
  table does not have, so every group deletion on a PostgreSQL panel failed
  with a database error. SQLite was unaffected.

- **PostgreSQL migration 11 no longer claims it will retry.** When it found
  conflicting ports it skipped itself, but the later migrations then recorded
  the newest schema version, so it never ran again despite the log saying it
  would. It now stops the panel with an explanation instead. (Not reachable in
  practice: the baseline schema applied just before already rejects such data.)

- **A re-sent traffic report is no longer billed twice.** When the panel had
  recorded a node's traffic but its answer never reached the node (a timeout,
  a dropped connection, a proxy error), the node sent those bytes again and
  they were billed a second time. Each report now carries a batch id; the
  panel records it in the same transaction as the traffic and acknowledges a
  batch it has already applied without billing it again. A node's next batch
  confirms the previous one, whose id is then kept one more day (a copy the
  node had already sent may still arrive late); an id nothing confirms (the
  node restarted, or is cut off) is kept for 30 days, so a copy re-sent after
  an outage shorter than that is still recognised. Needs node-v1.2.6 on
  the node as well — older nodes send no id and are billed as before; older
  panels ignore the id. PostgreSQL gets schema revision 28 (a new table);
  SQLite gets migration 45.

- **Every part of the panel's Chinese font loads again.** The build inlined
  a few of the font's smallest pieces into the stylesheet as `data:` URIs,
  which the panel's own Content-Security-Policy does not allow for fonts, so
  the browser refused them (a row of console errors) and the characters they
  cover fell back to a system font. Fonts are now always separate files, and
  CI fails if one is ever inlined again.

### Security

- **A short `JWT_SECRET` is now refused.** The panel only rejected an empty
  secret or the placeholder, so something like `JWT_SECRET=abc` was accepted.
  Login tokens are HS256-signed: anyone holding one issued token could
  brute-force a short secret offline and then sign an admin token. The panel
  now requires at least 32 characters and exits with an explanation otherwise.

  Installs made with `deploy.sh` generate a 64-character secret and are
  unaffected. If yours is shorter, `deploy.sh` — which the one-click update
  also runs — replaces it in `.env` with a new random one, so the upgrade
  cannot leave the panel down; everyone has to log in again once. If you run
  the compose file by hand, set a new one with `openssl rand -hex 32`.

- **Revoking a user's line authorization can no longer race with the user
  creating or resuming a rule.** The rule API checked the authorization
  before its write, and a revocation that landed in between paused the rules
  that existed at that moment — not the one being written, which came up
  running on a line the user no longer had (also by resuming a paused rule or
  moving one to another line). The write now checks the authorization again
  inside its own transaction, and on PostgreSQL both sides lock the user row
  first, so each waits for the other. An admin acting for a user is not
  checked, as before.

- **Release workflow permissions narrowed.** The panel release workflow gave
  every job write access to the repository and packages, including the job
  that runs `npm ci`. Jobs now get only the access they need, and the
  frontend build gets none. `:latest` on the panel image is moved only after
  the release is verified, as the node workflow already did.

- **The panel no longer runs as root in its container.** It runs as the
  unprivileged user `relaypanel` (uid/gid 10001), so a flaw in the panel no
  longer hands an attacker root inside the container — which matters more
  since the panel shares the host's `./run` directory with the one-click
  updater. The container still starts as root, but only for an entrypoint that
  gives `/app/data` and the `./run` mount point to `relaypanel` and then drops
  privileges with `setpriv`. That step is what lets an existing install
  upgrade in place: the database written by earlier root-run images is handed
  over on the first start, with nothing to do by hand. The node image is
  unchanged.

  If you bind-mount a host directory as `/app/data`, it will be owned by uid
  10001 on the host after the upgrade. When running the CLI with
  `docker compose exec`, add `-u relaypanel`.

- **Login and registration can no longer be used to exhaust the panel's CPU,
  and no longer reveal which usernames exist.**
  - Login attempts were limited only per username, so trying a different
    username each time was never limited. There is now also a limit per client
    IP (20 attempts a minute, login and registration together). Behind Caddy
    or a reverse proxy on the same host, the client IP is taken from the
    `X-Forwarded-For` header that proxy adds; a proxy that does not send it
    gets no per-IP limit rather than one counter shared by every user (add
    `proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;` to nginx).
    A reverse proxy on another host with a public address counts all its users
    as one client.
  - Password hashing ran on the threads that serve every request, so a few
    concurrent login attempts could stall the whole panel, node reports
    included. It now runs separately, at most one per two CPU cores at a time;
    when those are all busy, a login waits up to 3 seconds and then gets
    "server busy" — the rest of the panel keeps working.
  - A login with an impossibly long username or password is refused at once.
  - The stand-in hash checked for unknown usernames was malformed, so an
    unknown username was answered almost instantly and a known one only after
    the real password check — the response time told which usernames exist.
    Both now take the same time.

- **Release workflows no longer paste the manual-dispatch version into
  shell scripts.** It is passed as an environment variable and must be a plain
  `X.Y.Z`. Only people with write access to the repository could trigger this.

- **Frontend dependencies updated.** `npm audit` reported 9 advisories;
  react-router is now 7.18.4, and the rest were test and build tools. None was
  reachable in the panel: the React Router ones concern server rendering and
  navigation to untrusted paths, neither of which the panel does.

## [1.2.11] - 2026-09-27

Ships alongside `node-v1.2.5`; neither requires the other, and the config
protocol is unchanged at version 4. No database migration.

Upgrading: this release installs the one-click updater described below, so
**update this time the manual way** (`git pull --quiet && ./deploy.sh`); from
the next release on, "Update now" on the dashboard does it. Live node rates
need `node-v1.2.5`; older nodes keep showing their 10 s figure.

### Added

- **One-click panel update.** "Update now" on the dashboard now updates the
  panel instead of linking to the manual steps — like the nodes' one-click
  upgrade, for installations made with the install script (under
  `/opt/relay-panel`, on a systemd host).

  The panel cannot do this itself the way a node does: it runs in a container,
  and replacing a container's own image from inside requires mounting the
  Docker socket, which would hand the panel — a public web app with
  self-registration — root on the host. So the panel only asks. It drops a
  request file in `./run`, a directory shared with the host; a systemd path
  unit installed by `deploy.sh` notices it and runs `scripts/panel-updater.sh`
  as root, which does what an operator would: `git pull --ff-only` and
  `./deploy.sh`. A compromised panel can at most request an update to the
  official latest release — the host ignores the request's contents and pulls
  the official repository.

  With SQLite the panel is stopped and its database copied to
  `./backups/` first (the newest five are kept), because the new version may
  migrate the schema. PostgreSQL is not backed up automatically. The panel is
  unreachable for about half a minute; node forwarding is not affected. The
  outcome — including a failed `git pull`, a version pinned in `.env`, or a
  failed deploy (after which the panel is started again) — is shown on the
  dashboard with the end of the updater's log, and the full log is in
  `/var/log/relaypanel-updater.log`. The request is recorded in the audit log.

  Existing installations need one manual update (`git pull && ./deploy.sh`) to
  install the updater; until then the button keeps pointing at the manual
  steps and says so.

- **Node rates on the node-status page are now near real time.** The upload /
  download rate used to be a ~10 s average that changed only as often as the
  node's whole status report ran. A node running `node-v1.2.5` or later now
  pushes its NIC rate over the existing WebSocket every 2 s, the panel keeps it
  in memory (never in the database), and the page polls a small new endpoint,
  `GET /api/v1/nodes/live-rates`, every 2 s while it is open — pausing when the
  tab is in the background. CPU, memory, disk and cumulative traffic keep their
  existing cadence. The detail drawer now updates while it is open too; it used
  to show the figures from the moment it was clicked.

  The rate is machine-wide, the same measure as before, so it includes traffic
  that never passes through relay-node — iptables forwarding, other services.
  Older nodes keep showing the 10 s figure.

  Regular users see live rates only for the lines the page already shows them:
  the endpoint and the node summary now share one visibility rule (authorized
  and not hidden) instead of two copies of it.

### Changed

- **A GitHub link in the header**, at its right end, pointing to the
  repository. It is shown to every user, as the login page already links the
  same repository to everyone.
- **System settings are reordered**: site settings first, then basic settings,
  notifications, announcements and the audit log.

## [1.2.10] - 2026-09-24

Panel only, and a license release rather than a feature one: no code behaviour
changes. The node stays on `node-v1.2.4` — a node version is tagged only when
something node-side changed, and a license-only node release would prompt every
operator to upgrade (and drop connections) for nothing. The node source on
`main` is Apache-2.0 from this point, so the next node release ships under it.

No database migration. Pull the panel image and restart.

### Changed

- **The license is now Apache-2.0, replacing AGPL-3.0.** This applies from this
  release onward. Every release up to and including `v1.2.9` / `node-v1.2.4`
  remains available under AGPL-3.0 and always will — relicensing is not
  retroactive, and a fork of an earlier tag keeps the terms it was published
  under.

  What changes for you: the network-use clause is gone. Running a modified
  RelayPanel as a service no longer obliges you to publish your changes. Apache
  2.0 also grants an explicit patent license, which AGPL-3.0 covered only
  indirectly, and requires that modified files be marked as changed.

  No third-party dependency forced or blocked this: every Rust crate and npm
  package in the tree is permissive (MIT / Apache-2.0 / BSD / ISC / Zlib and
  friends), and the only AGPL entries a license scan reported were this
  repository's own three crates.

## [1.2.9] - 2026-09-08

Maintenance release. The node ships alongside as `node-v1.2.4`; neither
requires the other. No database migration, and the config protocol is unchanged
at version 4.

### Added

- **A node that fails to upgrade itself now says so.** Pressing "upgrade" sent a
  command and wrote an audit entry saying it had been dispatched — which was all
  the panel could honestly claim, because everything that can actually fail
  happens afterwards on the node (downloading the binary, verifying its sha256,
  backing up, swapping) and the node reported none of it back. The failure was
  written to the node's local log, on a machine whose whole problem is that
  nobody is watching it, so from the panel a failed upgrade and a slow one
  looked identical, permanently.

  A node running 1.2.4 or later now posts the failure to the panel, which
  records it as `upgrade_node_failed` in the audit log with the reason. Success
  still needs no report: the upgrade ends by restarting the node, which comes
  back and reports its new version in the ordinary status report.

  The endpoint is additive and authenticated by the node token, so the config
  protocol stays at version 4 and an older node simply never posts. The error
  text is node-controlled, so the panel folds control characters out of it and
  caps its length before it reaches the audit log.

### Fixed

- Rule creation and explicit transport updates now reject retired WS / TLS Simple
  ingress with an actionable error. Previously the API could save these rules
  successfully even though current nodes skip their listeners.
- Resuming a legacy WS / TLS Simple rule is rejected even when the request omits
  its transport. Existing rules can still be paused, renamed, or converted to
  raw TCP forwarding by clearing the old transport profile.
- API clients that omit `forward_mode` now get direct forwarding by default.
  Previously the default was the retired group mode, causing an otherwise valid
  rule-creation request to be rejected.
- Sending `tunnel_profile_id: null` now clears the rule's old transport profile.
  Omitting the field still preserves it; previously both requests preserved the
  binding, preventing API clients from converting legacy rules to raw transport.

## [1.2.8] - 2026-08-27

Panel only. The node ships separately as `node-v1.2.3` — take that one if you
sell traffic, it closes a billing hole. Neither release requires the other and
the config protocol is unchanged at version 4.

No database migration. Pull the panel image and restart.

### Changed

- **An audit entry about a device group records the group's NAME, not just its
  id.** "分组 4 → 1.2.2" only helps somebody who already knows which group 4
  is, and the log is meant to be read months later — by which point the group
  may have been renamed, or deleted and its id unresolvable. The name is now
  captured at write time, exactly as the actor's name already was, with the id
  kept beside it because names are neither unique nor stable. Applies to
  upgrading a node, clearing a node's status record, rotating a group token and
  deleting a group.

  Deleting a group needed the lookup moved ahead of the delete: afterwards the
  row is gone, and that is the one case where an id-only record can never be
  interpreted again.

- **A node upgrade is recorded as DISPATCHED, not completed.** The entry is
  written the moment the command reaches the node's control channel. Everything
  that can actually fail happens after that, on the node — downloading the
  binary, verifying its checksum, backing up, swapping — and the node does not
  report the outcome back, so a failed upgrade and a successful one produced
  identical rows. The action now reads 下发节点升级 and the detail points at the
  node's reported version, which is what actually answers "did it work".

## [1.2.7] - 2026-08-13

Panel only. The node ships separately as `node-v1.2.2`; neither release
requires the other and the config protocol is unchanged at version 4.

No database migration. Pull the panel image and restart.

**Read this one before upgrading if you sell plans**: a plan purchase now
enforces its rule limit against rules that ALREADY exist, so buying a smaller
plan pauses the buyer's newest excess rules immediately. See below.

### Changed

- **A plan's rule limit now applies to rules that already exist.** It was
  enforced only when creating a rule, so a user who downgraded kept every rule
  they had already made and the smaller plan bought them nothing. Buying a plan
  now keeps the oldest active rules and pauses the newest excess ones.

  The paused rules are marked like a manual pause on purpose: a later upgrade
  does **not** silently reopen those ports. The user resumes them when they want
  them back.

  Manual resume is held to the same limit — otherwise a downgrade could be
  undone one on/off switch at a time. Resuming past the limit now fails with a
  clear message instead of quietly succeeding.

  An admin editing `max_rules` directly on the user page is deliberately NOT
  reconciled: that path stays "applies to new rules only".

- **Registration copies the selected plan's line authorization**, not just its
  quotas. A user could previously register onto a plan and then find they were
  not authorized for any of the lines that plan advertises.

- **A failed database read is no longer reported as an empty list.** Admin list
  endpoints (rules, groups, users, plans, profiles, stats, node status) returned
  an empty success on a database error, so an outage looked exactly like "you
  have no rules" — and an operator acting on that would go and recreate things
  that still existed. They now return an explicit error, and the pages show a
  load-failure notice while keeping whatever was already on screen.

- **Rule filters live in the URL.** The group filter and the search box were
  component state, so a refresh dropped them and a filtered view could not be
  linked to anyone. `group` and `q` now join the existing `owner_uid`, so the
  whole filter set is one shareable address. Typing does not push a history
  entry per keystroke.

- **Row actions on the rules table name their rule.** Every row's buttons
  carried the same visible word, so a screen reader announced an
  undifferentiated list of them — including "delete". The visible labels are
  unchanged; only the accessible name is now qualified.

### Fixed

- **A rejected rule resume no longer wedges the SQLite writer.** The resume path
  opens a write transaction and returned early on error without rolling back,
  handing the pooled connection back while it still held SQLite's write lock.
  Every later write in the process then blocked and failed until the process
  restarted. Reachable from an ordinary edit: resuming a rule while also moving
  its port onto one already in use.

- **A failed rules read no longer empties the inbound-group picker.** The two
  lists were fetched together, so one failing discarded the other's result —
  leaving an admin unable to create a rule for a problem that had nothing to do
  with groups.

## [1.2.6] - 2026-08-12

Panel only. The node remains `node-v1.2.1`; this release does not change the
node protocol, forwarding runtime, installer, or node image.

No database migration is required. Pull the panel image and restart.

### Added

- **Notification settings now have explicit alert rules and delivery history.**
  Administrators can independently enable offline, recovery, node-version, and
  repeat-offline alerts. Recent per-channel delivery outcomes are retained and
  shown in the notification settings page, so a failed test or real alert has a
  visible reason instead of disappearing into logs.

### Changed

- **Node-table transfer rates stay on one line.** The upload/download column is
  given enough space on desktop layouts while less important columns contract
  first, keeping live throughput values readable without disruptive wrapping.

---

## [1.2.5] - 2026-08-02

Panel only. The node ships separately as `node-v1.2.1` and neither release
requires the other — the config protocol is unchanged at version 4, so any
current node works with this panel and vice versa.

No schema change and nothing to reconfigure: pull the image and restart.

### Changed

- **An offline node stays listed for 24 hours instead of 2 minutes.** The status
  row was deleted 120 seconds after a node went quiet, so combined with the 30s
  offline window a node was painted offline and then vanished a minute and a
  half later. The panel could not answer "which of my nodes is down right now",
  because during an outage the row worth looking at is exactly the one that had
  been removed.

  Past a day the row is still swept, so a decommissioned node disappears on its
  own; an admin who does not want to wait can now remove it by hand.

- **Text contrast raised across the panel.** `--rp-text-tertiary` sat at 3.02:1
  against white, below the 4.5:1 WCAG AA asks for body text, and it carried real
  content on the node table — CPU, memory and disk percentages, transfer rates.
  antd's own text tokens had never been set either, so table bodies fell back to
  `rgba(0,0,0,0.45)` at 2.85:1. Both now meet AA.

- **Offline nodes are greyed.** Every figure on an offline row — CPU, speed,
  uptime, connections — is the node's last report rather than a live reading,
  and rendered like an online row a wall of healthy-looking numbers said the
  opposite of what was true. The threshold colouring on its usage bars is
  neutralised for the same reason. Grey, not faded: those readings are what you
  look at to work out why the node went, so the row stays above the 4.5:1
  contrast floor. Applies to the mobile card list too.

- **A monitor-only group stops asking for forwarding settings.** It reports node
  status to admins and nothing else — no rule is bound to it, and
  `list_shared_groups` filters to `group_type='in'`, so it never reaches a
  regular user's lines or node status either. Connect host, port range, rate and
  hidden were still required on the form and shown in the table, which read as
  configuration when nothing consumed them. They are now dropped from the form
  and shown as `-` in the table. Converting an existing group to monitor leaves
  the stored values untouched, so switching back restores it intact.

- **A group's type reads as words, and the listener type is renamed.** The type
  column rendered the raw wire value, so an otherwise Chinese page showed "IN"
  and "MONITOR".

  It now shows the same label the form's picker offers — and that label changed:
  what was 入口（监听节点） is now 出口（监听节点）, because listening is a
  capability of that machine rather than a type of its own. Every 入口分组 in the
  UI follows. The legacy `out` type had to be renamed too: it was already called
  出口, and leaving it would have made one word mean two different things on the
  same page. It is now 落地（旧版，已废弃） — still labelled, because a database
  from an older version can still contain one, even though the type has not been
  offered when creating a group for several releases.

  English is named by ROLE rather than direction (Listener node / Listener
  Group), so the two languages do not assert opposite directions for one field.

  **Display only** — the stored values stay `in` / `out` / `monitor`. Renaming
  them would mean a migration, a protocol change and both repository
  implementations, to fix what was a wording problem.

### Added

- **Remove an offline node from the list.** Offered only on offline rows —
  deleting an online node's record achieves nothing, because its next report
  recreates it within seconds. The confirmation says plainly that this drops a
  record rather than uninstalling anything. The deletion is audited.

- **`relay-panel reset-admin-password [USER]`.** Recovering a lost admin
  password meant editing the database by hand: pasting a bcrypt placeholder
  full of `$` through a shell, which mangles under quoting, only worked for
  user id 1, and failed SILENTLY when the pattern did not match. Somebody
  locked out of their own panel is the last person who should be debugging
  shell escaping.

      docker exec relay-panel-panel-1 ./relay-panel reset-admin-password

  The password is generated, not taken from the arguments — anything passed on
  a command line lands in shell history and is visible to `ps` on a shared
  host. Sixteen characters with at least one of each class, drawn from the OS
  CSPRNG with rejection sampling so no character is likelier than another.
  Ambiguous glyphs (`0/O`, `1/l/I`) and shell-hostile ones (quotes, backslash,
  backtick, `$`) are excluded: this gets read off a terminal, retyped, and
  pasted through shells.

  Resetting signs out that account's existing sessions, which matters when the
  reason for the lockout is that somebody else got in. It does NOT force a
  password change on next login — the generated password is strong enough to
  keep — and it writes an audit entry so the reset is visible afterwards.

  USER defaults to `admin`; passing a name fixes the old id-1-only limitation.

---

## [1.2.4] - 2026-07-29

Panel only — no node release, and no node change is required (the config
protocol is unchanged at version 4, so existing nodes keep working as-is).

Three tables are created by migrations on first boot: node metric history, the
audit log, and announcements. Nothing to reconfigure, and nothing to migrate by
hand.

### Added

- **Announcements.** Post a notice with a title, a severity that sets the
  banner colour, an optional expiry and a pinned flag. One shows as a banner on
  the account page — the pinned notice, else the newest unexpired one — and
  every notice stays readable on an Announcements page open to any signed-in
  user, reached from a bell in the header that carries a dot when something is
  unread.

  Expiry is what lets "maintenance tonight" retire itself: past its date the
  notice leaves the banner but stays in the archive.

  The body accepts a small Markdown subset — bold, italic, code, links and
  lists — rendered as React elements rather than HTML, so operator-authored
  text can never become markup on a user's page. Links are restricted to
  http/https; anything else renders as the literal text typed.

  Announcements are visible to signed-in users only and never appear on the
  login page.

- **Audit log.** Destructive operations are recorded to the database:
  creating and deleting users, resetting passwords and traffic, assigning
  plans, deleting and restarting rules, deleting groups, rotating node tokens,
  upgrading nodes, generating / voiding / deleting / redeeming codes, and
  changing notification, site or announcement settings.

  "Who deleted my rule" previously had to be dug out of the process log, which
  rotates, dies with the container and is invisible from the panel. It is now
  queryable in the panel, filterable by action, and kept for a year.

  The actor's name is a snapshot taken at write time, so deleting that admin
  does not erase who did it. Details never contain a secret: rotating a token
  records how many nodes were disconnected, never the new token.

- **Site branding.** A site name and subtitle replace the hardcoded
  "RelayPanel" on the login page, in the sidebar and in the browser tab, plus a
  support contact shown on the account page.

- **Node CPU / memory / connection history.** Node status was a snapshot each
  report overwrote, so "why was it slow last night" had nothing behind it.
  Hourly rollups are now kept for 7 days and charted below the traffic chart.

  Sums and a sample count are stored rather than a running average, so the mean
  is exact at read time; maxima are kept alongside, because stalls are peaks and
  an hourly mean flattens exactly the spike being looked for.

- **Every user's purchases**, under plan management. The shop page shows a user
  their own orders; this is the operator's view of all of them.

- **Top-up history** on the account page. Codes are shown masked to the last
  group — that page gets screenshotted into support chats, and matching a
  receipt only needs those four characters.

- **Rule search** by name, listen port or target address, composing with the
  existing group filter.

### Changed

- **The admin menu is grouped into two submenus.** 用户与计费 holds 用户管理 /
  套餐管理 / 卡密管理; 系统设置 holds 基础设置 / 通知设置 / 站点设置 / 操作审计.

  Plans and redeem codes are deliberately NOT filed as settings: they are
  records you create and delete as routine work, not configuration you set once,
  and a daily CRUD page buried in "system settings" costs a click every time.

  Regular users are unaffected — their menu is unchanged.

- **Notification settings are their own page.** They were a second card on the
  system-settings page; the remaining registration card is retitled 基础设置 so
  the submenu does not read "系统设置 > 系统设置".

## [1.2.3] - 2026-07-26

Admin-console layout and a token-rotation button. Panel only — no node
release, no schema change, nothing to reconfigure: pull the image and restart.

### Added

- **Rotate a device group's node token from the UI.** The endpoint has existed
  since v0.3.9 but nothing ever called it. Rotation is a hard cutover, not a
  handover: the panel cannot push a new token to a node over the existing
  connection, because the token is exactly what that node authenticates with.
  Every node in the group is disconnected until re-enrolled, so the button
  states how many will drop, asks for the group name to be typed, and then
  hands back the new token together with the ready-to-paste enrollment command.
  Use it when a token has leaked.

### Changed

- **The forwarding tab now has one field width.** 负载策略 and 最大连接数 were
  full-width while 限速 and 自动重启间隔 came out around half that — the latter
  two carry addons, which antd wraps in a group that `width:100%` doesn't
  stretch. All four now line up. The target address is a little wider, sized to
  the most the row takes before the delete button wraps onto its own line.

- **The notification settings are laid out in columns.** Thirteen full-width
  fields in one stack meant a chat id got an 1800px input on a wide screen, and
  configuring email meant scrolling past Telegram. Global settings and Telegram
  now sit side by side, email spans the row beneath them, and each channel's
  on/off moved into its card header. Everything collapses to a single column on
  narrow screens.

### Docs

- **Troubleshooting is now maintained in one place — the site.** It had drifted
  into three places that didn't cross-link and covered different problems: a
  node showing offline had no answer on the site, and a panel failing with
  PoolTimedOut had none in the repo. The site now covers both panel deployment
  and node problems; `docs/DEPLOYMENT.md` and the node docs keep a short
  first-resort list and link out. Two entries that existed only in
  `DEPLOYMENT.md` were moved to the site rather than dropped.
- The READMEs are down to what someone needs to decide whether the panel fits:
  8 grouped feature bullets instead of 22, and the upgrade section no longer
  repeats what the node docs already say.

## [1.2.2] - 2026-07-25

Admin-console usability. Panel only — no node release, no schema change, and
nothing to reconfigure: upgrade the panel image and you're done.

### Added

- **Search users by username or ID** on the user-management page. The list was
  unfiltered, so finding one account on a populated panel meant scrolling. The
  filter runs client-side over the list already fetched, so it responds as you
  type without a round-trip.

### Changed

- **Redeem codes show WHO redeemed them, by username rather than `#id`.** An id
  is only useful if you already know who it is. The name is resolved server-side
  in one lookup per page (not one per row), and only when the page actually
  contains a used code. A code whose account was later deleted still shows the
  id — that row is the money-in record and outlives the account.

- **The redeem-code delete button is always visible**, disabled until rows are
  ticked. It previously appeared only *after* a selection was made, so with
  nothing selected the page looked like it had no way to delete at all — the
  feature existed and was simply invisible.

- **The target address field is wide enough for IPv6.** At its old width a full
  IPv6 literal was cut off mid-address, which is exactly when you need to read
  it carefully.

- **The load-balancing explanation moved into a "?" tooltip** on the strategy
  field. It used to be an always-open info panel — four lines of standing text
  for something you read once and then scroll past forever.

### Docs

- Node upgrade now leads with **one-click upgrade from the panel** instead of
  re-running the install script, which is the fallback path. Both node docs and
  both READMEs. Also documents two things that were missing everywhere: an
  upgrade **drops that node's live forwarding connections**, and one-click is
  **systemd-only** (Docker shows an "update the image" hint; a manually-run node
  has nothing to restart it).
- The feature highlights list features instead of implementation details, and
  covers the v1.2.0 additions that were missing from it.

## [1.2.1] - 2026-07-21

### Fixed

- **The panel no longer fails to start against a database whose
  `traffic_history` predates `group_id`.** The baseline schema re-runs on every
  boot and creates the table with `IF NOT EXISTS` — a no-op when the table is
  already there. It then indexed `group_id`, which on such a database runs
  *before* the migration that adds the column, so startup aborted with "column
  group_id does not exist" and the container crash-looped.

  The index now lives only in the migration that adds the column (SQLite
  Migration 41 / PG revision 24). Migrations run on fresh installs too, so it is
  still created exactly once either way — no schema version change.

  A **released** 1.1.3 deployment is not affected: `traffic_history` did not
  exist before 1.2.0, so an upgrade builds the table from the baseline with the
  column already present. This only bites a database carrying the intermediate
  shape, i.e. one running a pre-release build. Every fresh-install test passed
  throughout, which is exactly why this reached a tag; there is now a test that
  boots from the pre-`group_id` shape instead.

## [1.2.0] - 2026-07-21

### Added

- **Traffic is charted per LINE, stacked by inbound device group.** The chart
  answered "how much" but not "through which line" — with several lines at
  different billing rates, one total column can't tell you which one is burning
  the quota. Each bar now stacks one segment per line, with a legend.

  - **The line is stored as a SNAPSHOT on each history row, not joined at query
    time.** This table deliberately outlives its rules, so a join would drop the
    history of any deleted rule and "last 7 days" would quietly shrink. Same
    reasoning as `orders.plan_name`. A deleted line keeps its history under
    `#id` instead of vanishing.
  - **Row count is unchanged**: a rule belongs to exactly one inbound group, so
    the `(rule_id, hour_ts)` key still holds.
  - Existing history is **backfilled** from each rule's current group, so
    pre-upgrade hours don't all render as "unknown". Rows whose rule is already
    gone keep 0 — that attribution is unrecoverable, and inventing one would be
    a lie.
  - **Colors are a validated categorical palette**, fixed slot order so a line
    keeps its color when another is filtered out. Red is excluded (reserved for
    status, and it failed the adjacent-pair check against orange at ΔE 7.1 for
    normal vision); violet is excluded because it collides with the panel's
    indigo accent. Past 6 lines the tail folds into "Other" ranked BY VOLUME —
    a generated 7th hue would be indistinguishable under CVD.

- **Traffic history (24h / 7d / 30d).** `forward_rules.traffic_used` was a running
  total with no time dimension, so nobody could answer "how much did I use this
  week" or "which rule suddenly burned 500 GB last night" — the two questions
  that matter when usage looks wrong. Usage is now recorded hourly and charted
  on the dashboard (fleet-wide, with a per-rule drill-down) and on the account
  page (the user's own).

  - **The chart's primary series is BILLED traffic — the same number the quota
    deducts**, accumulated inside the same transaction as the charge. On a line
    with `rate != 1` the real and billed bytes genuinely differ, and a chart
    showing only real bytes would read as the panel over-charging. Real
    upload/download stay in the tooltip.
  - **One row per (rule, hour), written as an UPSERT.** Nodes report on the poll
    interval (~10s), so a row per report would be ~8.6k rows/rule/day; hourly
    accumulation keeps 100 rules × 35 days at ~84k rows.
  - **Day grouping happens in the VIEWER'S timezone, not the server's.** Buckets
    are stored in UTC, and a UTC "day" starts at 08:00 for a UTC+8 operator —
    grouping server-side would visibly misfile yesterday's traffic. The folding
    logic is a unit-tested pure function, including the UTC+8 case.
  - **No foreign key on the history table, deliberately.** Deleting a rule must
    not retroactively shrink "last 7 days". Rows die by retention (35 days,
    hourly sweeper) and nothing else.
  - Quiet hours are zero-filled rather than collapsed, so the axis can't imply
    continuous usage that didn't happen.

- **Node offline alerts (Telegram + email).** A node could die and nothing told
  anyone — the operator found out from a user complaint or by happening to
  refresh the panel. A background watcher now scans node status and notifies
  when a node has been silent past a threshold, and again when it recovers.

  - **The alert threshold is deliberately NOT the UI's online window.** The
    status dot flips after 30s, which is right for a status light and wrong for
    a page: a node that misses two reports on a flaky link is briefly "offline"
    and perfectly healthy. Alerting has its own threshold (default 180s ≈ six
    missed reports, floor 60s), and a compile-time assert keeps that floor above
    the online window so an alert can never fire for a node the status page
    still paints green.
  - **Each transition alerts exactly once.** An ongoing outage does not re-alert
    every tick; re-alerting is how a channel gets muted, and a muted channel is
    worse than none.
  - **State is in memory, like the auto-restart scheduler** — persisting "was
    offline" would replay every outage that happened while the panel was down,
    so an upgrade would open with a burst of stale pages. One case is handled
    explicitly rather than dropped: a node first seen ALREADY offline (it died
    during the restart) still alerts once, because that is exactly when the
    operator needs to know.
  - **Credentials are write-only.** The bot token and SMTP password go in
    through PUT and are never returned — GET reports only whether one is stored,
    and an empty credential on save means "keep the stored one", so the form
    round-trips without the browser ever holding the secret. They are stored in
    plaintext, same as node tokens: anyone who can read this database already
    controls the fleet, so encrypting one field beside the keys would be
    theatre.
  - **A "save & send test" button per channel**, because notification config is
    the classic write-and-forget setting — a typo'd chat id is invisible until
    the night a node actually dies. It saves first (the backend tests the STORED
    config) and surfaces the provider's own error text, so a failure is
    actionable rather than a generic "failed".
  - Enabling a channel without its required fields is refused up front, rather
    than silently never sending.

- **Redeem codes (balance top-up).** The shop could already DEDUCT balance
  (buying a plan) but nothing could ADD any — balance could only be typed in by
  an admin, so the selling flow was open-loop. An admin now generates batches of
  codes and a user redeems one to credit their own balance. No payment gateway,
  merchant account, or compliance work involved; codes can be sold or handed out
  offline.

  - **Redemption is exactly-once.** The claim is a conditional
    `UPDATE ... WHERE status = 'unused'` whose affected-row count is checked,
    inside the same transaction that credits the balance. Two concurrent
    redemptions of one code both attempt it, exactly one sees 1 row, and the
    loser rolls back without touching any balance. SQLite relies on writer
    serialization, PG adds `SELECT ... FOR UPDATE` — the mechanisms differ, so
    both backends are tested.
  - **Codes are made to be typed by a human.** Crockford Base32 (`I`, `L`, `O`,
    `U` excluded) in `XXXX-XXXX-XXXX-XXXX` form, 80 bits of entropy. Input is
    normalized before lookup: case, dashes, whitespace, and the classic `O`→`0`
    / `I`,`L`→`1` misreadings all resolve to the same code.
  - **A failed redemption never says why in a useful way.** "No such code" and
    "already used" return one identical message — distinguishing them would let
    a stranger brute-force codes and learn which guesses were real.
  - **Used codes are permanent.** They cannot be voided or deleted, and deleting
    the account that redeemed one nulls the reference but keeps the row
    (`ON DELETE SET NULL`): it is the record of money entering the system, so it
    outlives the account.
  - **Expiry is non-destructive.** An expired code is refused but stays
    `unused`, so an admin can extend a batch instead of regenerating it.
  - Crediting is refused if it would push a balance past `MAX_BALANCE` —
    deduction could never overflow, top-up can, and persisting an out-of-range
    balance would leave a value the panel can no longer write.

### Schema

- SQLite Migrations **39-41**, PG revisions **22-24** (`PG_SCHEMA_VERSION` 21 → 24):
  the `redeem_codes` and `traffic_history` tables, plus
  `traffic_history.group_id` (backfilled).

### Fixed

- **The redeem-code entry is now findable.** The control existed only on the
  account page, as a small text-link button beside the balance number — users
  reported there was no way to top up at all. It now also sits on the shop's
  balance card (where you actually discover you can't afford a plan, so you no
  longer have to leave the purchase flow), and reads as a button in both places.
  Redeeming re-reads the account rather than patching the number locally, so the
  displayed balance can't drift from what the backend recorded.
- **The traffic chart no longer appears twice for admins.** It was on both the
  dashboard (fleet-wide) and the account page (personal); an admin's "personal"
  traffic is a near-meaningless number, so the duplicate card was just noise.
  Regular users keep it on their account page — that is their only view.
- **The connection cap is no longer offered on UDP-only rules.** It is enforced
  at `accept()`, which UDP doesn't have, so the panel would store the number and
  ship it to the node where nothing reads it. The field is now disabled for a
  UDP-only rule with a note saying why; a `tcp_udp` rule keeps it (it governs
  the TCP half).
- **Batch restart no longer blames the wrong thing on partial failure.** It
  reused the batch-resume message ("unauthorized lines can't be resumed"), which
  has nothing to do with a restart — it now names the real causes (paused rule,
  or nodes offline / too old).

### Added

- **Rule restart (manual + batch).** `POST /rules/{id}/restart` drops every
  connection a rule is currently carrying and rebuilds its listeners on each
  node of its inbound group. Owner-scoped (a user may restart only their own
  rules); batch restart is the frontend calling it per rule, matching batch
  pause/resume, so there is deliberately no bulk endpoint. The rule's `paused`
  flag is never read or written — a restart is not a state transition. A paused
  rule is rejected rather than reported as a hollow success: it has no listener
  to restart, and the user's actual intent there is "resume".

  This is deliberately NOT implemented as pause+resume. That pair leaves the
  rule PAUSED if the resume half fails (node offline, authorization revoked
  between the two calls, panel restarted mid-way) — an outage caused by the
  button whose whole job is to end one. It also frees the listen port for
  auto-assignment during the gap, and writing `paused` resets `auto_paused`
  (v1.0.8), corrupting the system-paused vs. human-paused distinction.

  The response's `restarted` field counts nodes ACTUALLY reached and can be 0
  on an otherwise successful request (every node too old or offline), so the UI
  keys its message off that rather than the envelope code — a restart that
  silently did nothing would otherwise be undetectable.

- **Scheduled rule restart.** A rule with `auto_restart_minutes > 0` has its
  connections dropped on that interval. The `max_connections` cap is the actual
  fix for connection accumulation; this is the valve for when you'd rather shed
  than refuse.

  The schedule lives in MEMORY, not the database. Persisting `last_restart_at`
  would mean every rule whose interval elapsed while the panel was down comes
  due at once on boot — a panel upgrade would begin by dropping every
  auto-restart rule's connections simultaneously. In-memory re-bases each timer
  to "now" on restart; the cost is at most one skipped cycle, which is invisible
  next to an unscheduled mass disconnect. A rule seen for the first time is
  baselined, never restarted on the spot.

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

### Compatibility

- Nodes below **1.2.0** silently ignore the unknown `restart_rule` message. The
  panel gates on `node_supports_restart_rule` and surfaces those nodes as
  "upgrade required" rather than counting them as restarted — a restart that
  quietly did nothing would be undetectable to the operator. Node Status already
  offers one-click upgrade.

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
