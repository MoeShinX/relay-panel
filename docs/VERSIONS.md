# Version sync checklist

RelayPanel ships **two independent version tracks** since v1.2:

- **Panel** — the management API + web UI. Tagged `vX.Y.Z`.
- **Node** — the `relay-node` forwarding binary. Tagged `node-vX.Y.Z`.

The two version numbers **do not have to match**. A panel-only update (new UI,
API, billing logic) does NOT rebuild or republish the node, and a node-only
update (forwarding fix, new arch) does NOT touch the panel. Each track has its
own pre-flight check, its own GitHub Release, and its own changelog.

This file is the single source of truth for where each track's version number
lives. When cutting a release, update only that track's locations, then run its
pre-flight check.

---

## Panel version (4 places)

| # | File | What to change |
|---|------|----------------|
| 1 | `crates/panel/Cargo.toml` | `version = "x.y.z"` |
| 2 | `crates/panel/src/config.rs` | `COMPILED_APP_VERSION` (the panel's own version, shown in the update-check UI). Overridable at runtime via the `APP_VERSION` env var. |
| 3 | `docker-compose.release.yaml` | the panel image tag — `RELAYPANEL_PANEL_TAG` default (`ghcr.io/moeshinx/relay-panel-panel:x.y.z`) |
| 4 | `README.md` / `README.en.md` | dynamic `github/v/release` badge (auto-reflects the latest panel release; no static string to bump, just confirm the badge exists) |

Also bump, but not part of the "must match" set:
- `CHANGELOG.md` — add a new `## [x.y.z] - YYYY-MM-DD` section describing the
  panel release. The section MUST be non-empty: `release-check.sh` + the panel
  Docker Release workflow extract it via `scripts/extract-changelog.sh` to build
  the GitHub Release body, and both FAIL on a missing / empty section.
- `Cargo.lock` carries `relay-panel`'s version, so run `cargo check` after bumping.

## Node version (4 places)

| # | File | What to change |
|---|------|----------------|
| 1 | `crates/node/Cargo.toml` | `version = "x.y.z"` (read by `relay-node --version` via `env!("CARGO_PKG_VERSION")`) |
| 2 | `scripts/relay-node-install.sh` | `SCRIPT_VERSION="x.y.z"` (the default version the install script pulls) |
| 3 | `docker-compose.release.yaml` | the node image tag — `RELAYPANEL_NODE_TAG` default (`ghcr.io/moeshinx/relay-panel-node:x.y.z`) |
| 4 | `CHANGELOG-NODE.md` | a `## [x.y.z] - YYYY-MM-DD` section (the node release body source) |

Also bump:
- `Cargo.lock` carries `relay-node`'s version, so run `cargo check` after bumping.

---

## How to verify they're in sync (per track, automated)

Run the track-specific pre-flight check — it verifies ONLY that track's
locations PLUS file existence, doc content, script sizes, and permissions:

```bash
bash scripts/release-check.sh panel 1.1.1   # panel release (tag v1.1.1)
bash scripts/release-check.sh node 1.1.0    # node release (tag node-v1.1.0)
```

For backwards compatibility, a bare version defaults to panel:

```bash
bash scripts/release-check.sh 1.1.1         # == panel 1.1.1
```

Expect 0 FAIL (warnings about the shared crate version are OK — `relay-shared`
is intentionally NOT release-sync'd). If any FAIL appears, fix it before
tagging.

---

## Release flow — PANEL (tag `vX.Y.Z`)

1. Update the 4 panel-version places above + `CHANGELOG.md`.
2. Run the full gates: `cargo fmt --check`,
   `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`, and (in `frontend/`) `npm run lint` **and**
   `npm run build`.
3. Run the panel pre-flight check and confirm 0 FAIL:
   ```bash
   bash scripts/release-check.sh panel X.Y.Z
   ```
4. Commit ("release: vX.Y.Z") and push to `main`. Wait for main CI green.
5. Tag on the current `main` HEAD: `git tag -a vX.Y.Z -m "..." && git push origin vX.Y.Z`.
6. The `v*` tag triggers **docker-release.yml** (PANEL-ONLY): it builds + pushes
   `ghcr.io/moeshinx/relay-panel-panel:X.Y.Z` + `:latest` (multi-arch manifest
   with `linux/amd64` and `linux/arm64` legs), and creates the panel GitHub
   Release (body from CHANGELOG.md). **No node artifact is built or pushed.**
   `relay-panel-node:latest` is untouched.

## Release flow — NODE (tag `node-vX.Y.Z`)

1. Update the 4 node-version places above + `CHANGELOG-NODE.md`.
2. Run the gates (the node crate + workspace tests):
   `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`.
3. Run the node pre-flight check and confirm 0 FAIL:
   ```bash
   bash scripts/release-check.sh node X.Y.Z
   ```
4. Commit ("node-release: node-vX.Y.Z") and push to `main`. Wait for main CI green.
5. Tag: `git tag -a node-vX.Y.Z -m "..." && git push origin node-vX.Y.Z`.
6. The `node-v*` tag triggers **node-release.yml** (NODE-ONLY): it builds the
   static musl amd64 + arm64 binaries (with sha256 sidecars), builds + pushes
   `ghcr.io/moeshinx/relay-panel-node:X.Y.Z` + `:latest`, creates the node
   GitHub Release (body from CHANGELOG-NODE.md), and verifies the assets +
   image version. **No panel artifact is built or pushed.**

---

## When NOT to cut a node release

A node release is only tagged when something node-side actually changed
(forwarding, the self-updater, the reporter, node CLI, or a node dependency).
A panel-only change (UI, API, billing, plans) must NOT produce a new `node-v*`
tag, must NOT republish identical node binaries, and must NOT move
`relay-panel-node:latest`. The split CI (above) enforces this: only a `node-v*`
tag builds node artifacts.

---

## How the panel tells nodes apart from itself (v1.2)

`GET /api/v1/system/version` returns:

- `current_version` — this panel's version.
- `latest_version` — the newest **panel** release (highest `v*` tag).
- `latest_node_version` — the newest **node** release (highest `node-v*` tag).
- `node_version_check_failed` — true if the node-version lookup failed.

The node-status UI compares each node's `node_version` against
`latest_node_version` (NOT the panel version), so a panel upgrade never makes a
current node look outdated. The directed node-upgrade command targets
`latest_node_version` too — a panel-only release can never command a node to
download a non-existent asset.

---

## Historical note (≤ 1.1.0)

Through 1.1.0, panel and node shared a single `vX.Y.Z` tag and were forced to
the same version. v1.2 split them. Node self-upgrade download URLs fall back to
the legacy `v*` path for versions ≤ 1.1.0 (where those binaries were originally
published); 1.1.1+ use `node-v*` exclusively.

---

## One-time cross-track upgrade bridge (only for the first `node-v*`)

A node running ≤ 1.1.0 uses the PRE-SPLIT updater, which downloads its upgrade
binary from the joint `v{version}` tag. Once the panel points those nodes at the
first node release (1.1.1), they fetch `v1.1.1/relay-node-linux-*` — but `v1.1.1`
is a PANEL-only release with no node binary, so the one-click upgrade 404s. Those
old nodes must either be re-installed via `relay-node-install.sh`, OR the
`node-v1.1.1` binaries can be copied onto the `v1.1.1` tag so the old URL resolves.

**Order is mandatory and must not be reversed.** Do this only AFTER `node-v1.1.1`
has finished its own `draft → verify → promote-latest → publish-release` flow AND
the panel `v1.1.1` release exists:

```
# after node-v1.1.1 is PUBLISHED (green) and v1.1.1 exists:
gh workflow run bridge-node-asset.yml -f version=1.1.1
```

`bridge-node-asset.yml` copies the ALREADY-VERIFIED `node-v1.1.1` assets onto
`v1.1.1` — it never compiles a fresh binary, refuses to run while `node-v1.1.1`
is still a draft, and re-verifies the sha256 before attaching. This preserves the
"invisible until verified" gate that unaware old nodes rely on. From 1.1.1 onward
nodes use `node-v*` and never need the bridge again — this is a one-shot step.
