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
