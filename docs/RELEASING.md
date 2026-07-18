# Releasing Ceiling

Ceiling releases are built from a clean, protected `main` branch and published
from an immutable version tag. Do not build public artifacts from a dirty
working tree.

## Prepare

1. Choose the next version and update `version.env`, both Rust manifests,
   `apps/desktop-tauri/package.json`, and `tauri.conf.json`.
2. Move the relevant entries from `Unreleased` into a dated changelog section.
3. Run the complete local validation:

   ```powershell
   powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -All -Version <version>
   ```

4. Open a pull request and wait for the hosted `Frontend` and `Rust` checks.
5. Merge with squash after resolving every review conversation.

## Build, sign, and smoke-test

Create an annotated `v<version>` tag from the merged `main` commit and push it.
`.github/workflows/release.yml` then:

1. builds the desktop and CLI from the immutable tag;
2. signs both binaries through Azure Artifact Signing and GitHub OIDC;
3. packages and signs the Inno Setup installer;
4. verifies the publisher and RFC 3161 timestamps;
5. regenerates SHA-256 sidecars after signing;
6. installs, validates, and uninstalls the signed build; and
7. creates or updates a draft GitHub release.

The GitHub `release` environment accepts only `v*` tags. Its federated identity
is `repo:tsouth89/ceiling:environment:release`; the workflow uses environment
variables for Azure resource identifiers and does not store an Azure secret.

For a local unsigned packaging rehearsal, use the managed Windows checkout:

```powershell
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\windows-release-build.ps1 `
  -Ref v<version> `
  -SmokeInstall
```

Do not publish that local rehearsal unless its app, CLI, portable executable,
and installer have been independently signed and
`scripts\finalize-windows-release.ps1` passes.

The release directory must contain:

- `Ceiling-<version>-Setup.exe`
- `Ceiling-<version>-Setup.exe.sha256`
- `Ceiling-<version>-portable.exe`
- `Ceiling-<version>-portable.exe.sha256`

Test install, launch, tray behavior, provider refresh, the capacity strip,
autostart, and uninstall on a clean Windows user profile before publishing.

### Build cache

To keep the signed path fast, the Cargo dependency sources (`~/.cargo/registry`,
`~/.cargo/git`), the release-mode Cargo target directories, and the pnpm store
are cached with a pinned `actions/cache`, keyed by runner OS, MSVC toolchain,
resolved `rustc` version, `Cargo.lock`, and `pnpm-lock.yaml`, with prefix
restore-keys so a small dependency bump still reuses most artifacts.

GitHub Actions caches are **ref-scoped**: a tag run cannot read another tag's
cache, and only default-branch runs can write the scope every tag run can read.
So the cache is **warmed on `main`** by `.github/workflows/warm-release-cache.yml`
(on pushes that change the build graph, a weekly schedule, and manual dispatch);
`release.yml` only **restores** it. The warm workflow does the same unsigned
build and saves the cache — it does no signing and touches no secrets.

Safety properties:

- The release (tag) run only restores; it never writes a cache, so no signed
  binary, credential, signing material, or release asset is ever cached.
- The warm workflow saves only after a successful build (`success()`-gated), so
  a failed build never poisons the cache, and it saves before it would ever have
  produced a signed artifact (it never signs at all).
- A cache miss falls back to a full cold build (the original behavior), so
  caching can only make a release faster, never break it.

The release run summary reports whether the build was warm, partial, or cold;
per-phase timing is visible as the individual step durations. The first release
after a build-graph change may be partial/cold until the next warm run repopulates
the default-branch cache.

Invalidation and no-cache recovery:

- To force a clean rebuild, bump `$cacheVersion` in the **Compute release cache
  key** step of *both* `release.yml` and `warm-release-cache.yml` (keep them in
  sync) and push a new tag.
- To purge stored caches, delete them from the repository's Actions cache UI or
  with `gh cache delete --all` (or a specific key). A release with no cache
  simply cold-builds.
- To warm on demand, run the **Warm release cache** workflow via *Run workflow*
  (workflow_dispatch) on `main`.

> Follow-up (evaluated, not yet applied): the build script uses separate Cargo
> target directories for the desktop and CLI builds, so the shared dependency
> graph compiles twice even on a warm run. Unifying them into one target
> directory would cut cold-build time and roughly halve the cache size; it is a
> build-script change worth measuring against the timing this cache change now
> records.

## Publish

1. Review the draft release created by the signed workflow and replace generated
   notes with concise, user-facing notes where needed.
2. Download and manually launch the signed installer and portable executable on
   a clean Windows profile.
3. Run `scripts\release-doctor.ps1 -Version <version>` and resolve all failures.
4. Publish the GitHub draft only after the manual checks pass.
5. Publish a new Winget package identity for Ceiling. Do not reuse the
   `Finesssee.Win-CodexBar` package identifier or product code.
6. Verify the GitHub download URL and SHA-256 before submitting the immutable
   Winget manifest.

## First-release gate

- The installer shows Ceiling everywhere and installs to `Programs\Ceiling`.
- No public artifact or support link points to Win-CodexBar.
- `main` is protected and the hosted checks are required.
- GitHub Issues, private vulnerability reporting, Dependabot alerts, and
  security updates are enabled.
- The README states the fork lineage and retains the upstream MIT notice.
- There are no startup notification replays or false depletion/restoration alerts.
- The five core providers have a documented happy path and a truthful failure state.
