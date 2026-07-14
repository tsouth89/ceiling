# Ceiling CI and release automation

Ceiling runs hosted frontend and Rust validation for pull requests and builds
signed Windows releases from immutable version tags.

## Pull request checks

`.github/workflows/ci.yml` runs the frontend test/build suite and both Rust test
and clippy suites. Before opening a pull request, run the smallest local check
that covers the change:

```powershell
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -Format -Clippy
powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -All -Version 0.43.2
```

## Signed release flow

1. Merge the release PR into protected `main` after all required checks pass.
2. Create and push an annotated tag that exactly matches the repository version,
   for example `v0.43.2`.
3. The `Signed Windows Release` workflow builds, signs, verifies, smoke-tests,
   and uploads four assets to a draft GitHub release.
4. Review the draft notes and manually test the downloaded installer and
   portable executable on a clean Windows profile.
5. Run `scripts\release-doctor.ps1 -Version <version>` against the downloaded
   assets, then publish the draft.
6. Submit the new Ceiling Winget manifest only after the GitHub asset URLs and
   hashes are stable.

The workflow uses Azure workload identity federation. The GitHub `release`
environment is restricted to `v*` tags and holds only non-secret Azure resource
identifiers. Azure issues a short-lived token for the environment-specific OIDC
subject, so there is no client secret or certificate private key in GitHub.

## Release script phases

The signed workflow deliberately separates compilation from packaging:

```powershell
.\scripts\windows-release-build.ps1 -Ref v0.43.2 -BuildOnly
# Sign ceiling.exe and codexbar-cli.exe.
.\scripts\windows-release-build.ps1 -Ref v0.43.2 -PackageOnly
# Sign Ceiling-0.43.2-Setup.exe.
.\scripts\finalize-windows-release.ps1 -Version 0.43.2
```

`-BuildOnly` prevents unsigned binaries from being packaged. `-PackageOnly`
reuses those signed binaries, and finalization refuses to generate publishable
hashes unless every public executable has the expected timestamped signature.
