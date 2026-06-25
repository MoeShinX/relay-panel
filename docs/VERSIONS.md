# Version sync checklist

This is the **single source of truth** for every place a version number lives
in the repo. When cutting a new release, every entry below MUST be updated to
the same version. The release is not done until all of them match.

Current target: **1.0.1**

---

## Where the version appears (7 places)

| # | File | What to change | Current |
|---|------|----------------|---------|
| 1 | `crates/node/Cargo.toml` | `version = "x.y.z"` (read by `relay-node --version` via `env!("CARGO_PKG_VERSION")`) | 1.0.1 |
| 2 | `scripts/relay-node-install.sh` | `SCRIPT_VERSION="x.y.z"` (decides which release binary to download) | 1.0.1 |
| 3 | `docker-compose.release.yaml` | both image tags: `ghcr.io/moeshinx/relay-panel-panel:x.y.z` AND `.../relay-panel-node:x.y.z` | 1.0.1 |
| 4 | `crates/panel/src/config.rs` | `COMPILED_APP_VERSION` (the panel's own version, shown in the update-check UI). Overridable at runtime via the `APP_VERSION` env var. | 1.0.1 |
| 5 | `README.md` | the `**Version:** \`x.y.z\`` badge line | 1.0.1 |
| 6 | `README.md` | the `**当前版本：** \`x.y.z\`` badge line | 1.0.1 |
| 7 | `crates/panel/Cargo.toml` | `version = "x.y.z"` (the panel crate version). `release-check.sh` FAILs if it drifts. `Cargo.lock` carries it too, so run `cargo check` after bumping. | 1.0.1 |

Also bump, but not part of the "must match" set:
- `CHANGELOG.md` — add a new `## [x.y.z] - YYYY-MM-DD` section describing the
  release. (Old sections are history and intentionally keep their old versions.)
  The section MUST be non-empty: `release-check.sh` + the Binary Release
  workflow extract it via `scripts/extract-changelog.sh` to build the GitHub
  Release body, and both FAIL on a missing / empty section (this prevents the
  release body extraction bug).

---

## How to verify they're in sync (automated)

Run the release pre-flight check — it verifies all 7 locations above PLUS file
existence, doc content, script sizes, and permissions:

```bash
bash scripts/release-check.sh X.Y.Z   # or vX.Y.Z
```

Expect 0 FAIL (warnings about panel/shared crate versions are OK). If any FAIL
appears, fix it before continuing — do not tag with a failing check.

For a quick manual grep (e.g. hunting for a leftover old version):

```powershell
$v = '1.0.1'  # the version you are migrating FROM
Get-ChildItem -Recurse -File -Include *.toml,*.sh,*.yaml,*.yml,*.md,*.rs,Dockerfile |
  Where-Object { $_.FullName -notmatch '\\(target|node_modules|\.git)\\' -and $_.Name -ne 'CHANGELOG.md' -and $_.Name -ne 'Cargo.lock' -and $_.Name -ne 'VERSIONS.md' } |
  Select-String -Pattern $v -SimpleMatch
```

Then regenerate `Cargo.lock` (it carries the node version):

```bash
cargo check -p relay-node   # refreshes Cargo.lock
grep -A1 'name = "relay-node"' Cargo.lock   # should show the new version
```

---

## Release flow (correct order)

This order matters — it avoids a mistake where the tag ended up
behind `main` by a commit:

1. Update all 7 places above + CHANGELOG.
2. Run the full gates: `cargo fmt --check`, `cargo clippy --workspace
   --all-targets -- -D warnings`, `cargo test --workspace`,
   `bash scripts/deploy-web-mode-check.sh`, and (in `frontend/`) `npm run lint`
   **and** `npm run build`. (Both lint and build must be checked — CI runs lint,
   which tsc alone does not.)
3. **Run the release pre-flight check** and confirm 0 FAIL:
   ```bash
   bash scripts/release-check.sh X.Y.Z   # or vX.Y.Z
   ```
   This verifies all 7 version locations agree, required files exist, docs
   contain the right keywords, and scripts are non-empty + executable.
   Warnings (e.g. panel/shared crate version not synced) are allowed; FAILs
   are not. Fix every FAIL before continuing.
4. Commit ("release: vX.Y.Z") and **push to `main`**.
5. **Wait for `main` CI to go green** (CI / Debian Compat / Script Check).
   Do NOT tag until main CI passes.
6. Create the tag on the current `main` HEAD: `git tag -a vX.Y.Z -m "..."` and
   `git push origin vX.Y.Z`. This triggers Binary Release + Docker Release.
7. Wait for both Release workflows to finish; verify the release assets
   (`relay-node-linux-amd64` + `-arm64`) are `uploaded` and the GHCR images
   `:X.Y.Z` exist.

---

## Why two version constants?

- `crates/node/Cargo.toml` version → read by `relay-node --version`
  (`env!("CARGO_PKG_VERSION")`). This is the **node** version.
- `crates/panel/src/config.rs` `COMPILED_APP_VERSION` → the **panel** version,
  shown in the dashboard's update-check banner.

They are kept identical because panel + node are released together, but they
are technically independent (the panel could update without a node binary
change, and vice versa). If they ever diverge, document which is which here.

---

## Pre-release vs stable

All releases so far are marked `prerelease: true` on GitHub. The update check
(`crates/panel/src/api/system.rs`) sets `ALLOW_PRERELEASE_UPDATES = true`, so
the dashboard will offer pre-release updates. Once you want to disable pre-release updates, flip
that constant to `false` to only notify about stable releases.
