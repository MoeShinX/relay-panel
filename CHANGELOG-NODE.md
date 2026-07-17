# Changelog — relay-node

All notable changes to the **relay-node** binary are documented here. This is a
SEPARATE changelog from `CHANGELOG.md` (which covers the panel + cross-cutting
features): panel and node release on independent version tracks (`node-vX.Y.Z`
tags vs panel `vX.Y.Z` tags), so each has its own history. A node release's
GitHub Release body is extracted from this file by
`scripts/extract-changelog.sh <version> CHANGELOG-NODE.md`.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

---

## [Unreleased]

### Added

- **`restart_rule` control message.** The panel can ask the node to drop one
  rule's connections and rebuild its listeners. The node re-creates listeners
  from its OWN cached config (`ForwarderManager::last_config`), never from
  anything in the message, so a restart cannot be used to inject listener
  config. `node_id` is re-checked on arrival as defence in depth even though
  `send_node` already routed it.

  The WS dispatch arm for this MUST stay above the diagnose arm and MUST check
  `type`: `DiagnoseRuleMessage` defaults its `challenge` field and ignores
  unknown fields, so a `restart_rule` payload deserializes into it cleanly
  (`rule_id` + `request_id` are both present). Ordered after diagnose, every
  restart would silently become a target probe instead. A test in
  `relay-shared` pins the ambiguity.

- **Per-rule concurrent TCP connection cap** (`ListenerConfig.max_connections`,
  None = unlimited). Admission happens in the accept loop, not in the spawned
  connection task: the accept loop is sequential, so check-then-increment there
  is exact, whereas incrementing inside the task would let an unbounded number
  of accepts through before the first increment landed — precisely the
  connection-flood case the cap exists for. Over the cap, the socket is dropped
  immediately (the "at cap" warning is rate-limited to once per 60s per
  listener; a rule sitting at its cap rejects on every accept, and an
  unthrottled warn would itself become the outage).

  The counter lives per RULE, not per listener: a dual-stack rule runs two
  accept loops (IPv4 + IPv6), and a per-listener counter would silently grant
  double the configured cap.

### Fixed

- **Aborting a listener no longer leaves its connections forwarding.**
  Connections run on detached `tokio::spawn` tasks, so an aborted accept loop
  stopped new accepts while every established connection kept relaying —
  verified: a post-abort read/write round-trips fine. Connections now select on
  a per-rule cancellation channel (`forwarder::gate`). Consequences:
  - an explicit `restart_rule` genuinely sheds connections rather than just
    re-binding the port;
  - a rule removed from the node's config now stops forwarding, instead of
    relaying bytes for a rule whose traffic counters `apply_config` already
    pruned.

  `apply_config`'s fingerprint-driven restart (changed targets / rate caps) does
  NOT cancel: editing a rule must not kick everyone off, which has been the
  behaviour since v0.3.6.

- **A UDP-only rule's restart is no longer a silent no-op.** `restart_rule`
  returned early when the rule had no `RuleRuntime` — but only the TCP arm of
  `apply_config` creates one (UDP has no `accept()` and no cancellable
  per-connection tasks), so a UDP-only rule has no runtime while very much
  having a listener. It was never torn down or rebuilt, and its sessions never
  dropped. The panel reports success as soon as the command reaches the node, so
  the operator was told the rule had restarted while nothing happened at all.
  "No runtime" now means only "no connections to cancel"; whether there are
  listeners to rebuild is decided separately.

### Compatibility

- Requires panel **1.2.0+** to be sent `restart_rule` or a connection cap. Both
  additions are backward compatible on the wire (`#[serde(default)]`), so a
  1.2.0 node runs against an older panel unchanged — it simply never receives
  either.

## [1.1.2] - 2026-07-12

### Fixed

- **UDP forwarding now follows DDNS target IP changes.** A UDP rule's domain
  target was resolved ONCE when the listener started (rule push / node boot) and
  the resolved IP was reused forever — so a DDNS target (WireGuard, game relay,
  DNS forwarding) that changed IP kept getting blackholed to the stale address
  until the rule or node was manually restarted. New UDP sessions now resolve
  through the shared 30s DNS cache (same as TCP), so an IP change is picked up
  within the cache TTL; established sessions age out on the 60s idle timeout and
  the next datagram opens a fresh session against the current IP. This also
  removes the old "unresolvable-at-boot kills the listener → restart loop"
  behavior — a transient DNS failure no longer tears down the UDP listener.

## [1.1.1] - 2026-07-08

### Fixed

- **File-descriptor exhaustion under connection churn.** Forwarded TCP sockets
  (both the accepted client side and the dialed target side) now enable TCP
  keepalive (idle 60s, 15s probes, 4 retries). Previously a peer that vanished
  without a FIN/RST — NAT rebind, mobile handoff, cable pull, a firewall that
  drops instead of resets — left the bidirectional copy blocked on `read()`
  forever, holding two fds; under churn these dead half-open connections
  accumulated until the node hit `EMFILE` ("Too many open files", os error 24),
  even at `LimitNOFILE=65536`. Keepalive lets the kernel reap dead peers so the
  copy task ends and releases its fds.
- **Low fd limit on non-systemd launches.** The node now raises its own
  `RLIMIT_NOFILE` soft limit toward the hard limit at startup, so a docker or
  manual (bash/nohup) launch — which inherits the 1024 default instead of the
  systemd unit's `LimitNOFILE=65536` — no longer exhausts descriptors under
  moderate load. The node Docker Compose service also sets `ulimits.nofile` to
  65536 to match.

---

## [1.1.0] - 2026-07-02

The node half of the **one-click remote upgrade** release. (Panel-side changes
for the same feature are in `CHANGELOG.md` under [1.1.0].)

### Added

- **Self-upgrade.** On receiving a directed `upgrade_node` command over the WS
  control channel, a systemd node downloads the official `relay-node` release
  for its architecture from the GitHub release, **verifies the published
  sha256**, backs up its current binary, atomically swaps, and exits so systemd
  restarts it. Safety:
  - **Upgrade-only:** the target must be a valid semver strictly newer than the
    running version, so a compromised panel can't force a downgrade.
  - **Install-aware:** only systemd nodes self-upgrade; docker nodes are told to
    update the image, and manual runs are disabled (nothing would restart them).
  - **Single-flight + mandatory backup:** repeated commands can't corrupt the
    binary, and a failed backup aborts the swap.
- Binaries continue to ship for both **amd64 and arm64** (static musl + rustls).

### Notes

- Assets for 1.1.0 and earlier were published under the joint `v*` tag (panel
  and node shared a release). From 1.1.1 onward, node binaries publish under the
  dedicated `node-v*` tag. The node's self-upgrade download logic falls back to
  the `v*` URL for versions ≤ 1.1.0 so existing 1.1.0 nodes can still reach the
  historical asset; newer versions use `node-v*` exclusively.

---

_The node has no code, forwarding, protocol, or dependency changes in this
round, so no newer `node-v*` version is cut. A node release is only tagged when
something node-side actually changed._
