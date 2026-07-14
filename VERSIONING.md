# Ceiling versioning

Ceiling uses [Semantic Versioning 2.0.0](https://semver.org/) and publishes `vX.Y.Z` release tags from protected `main`.

## Choosing a version

- **Patch (`x.y.Z`)**: backward-compatible fixes and small polish.
- **Minor (`x.Y.0`)**: new providers, user-facing features, or substantial new surfaces.
- **Major (`X.0.0`)**: incompatible settings, CLI, data, or platform changes.

Use `-alpha.N`, `-beta.N`, or `-rc.N` only when a build is intentionally pre-release.

## Version locations

A release version must match in every active location:

| File | Field |
| --- | --- |
| `version.env` | `MARKETING_VERSION` |
| `rust/Cargo.toml` | package `version` |
| `apps/desktop-tauri/src-tauri/Cargo.toml` | package `version` |
| `apps/desktop-tauri/package.json` | `version` |
| `apps/desktop-tauri/src-tauri/tauri.conf.json` | `version` |

`Cargo.lock` must be regenerated and committed after changing either Rust manifest.

## Release flow

1. Update every version location and move relevant changelog entries into a dated release section.
2. Run `powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -All -Version <version>`.
3. Merge the version bump through a pull request after the required Frontend and Rust checks pass.
4. Create and push an annotated `v<version>` tag from the merged `main` commit.
5. Let `.github/workflows/release.yml` build, sign, smoke-test, and create the draft release.
6. Manually test the signed installer and portable build before publishing the draft.

See [docs/RELEASING.md](docs/RELEASING.md) for the complete signing, validation, and Winget checklist.
