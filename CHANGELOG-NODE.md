# Changelog — relay-node

All notable changes to the **relay-node** binary are documented here. This is a
SEPARATE changelog from `CHANGELOG.md` (which covers the panel + cross-cutting
features): panel and node release on independent version tracks (`node-vX.Y.Z`
tags vs panel `vX.Y.Z` tags), so each has its own history. A node release's
GitHub Release body is extracted from this file by
`scripts/extract-changelog.sh <version> CHANGELOG-NODE.md`.

The format is based on [Keep a Changelog](https://keepachangelog.com/).

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
