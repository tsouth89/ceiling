## Summary

Describe what changed and why.

## Related issue

Fixes #

## Affected areas

Check every area this PR changes or could affect:

- [ ] Tray panel
- [ ] Settings UI
- [ ] Config file / settings persistence
- [ ] CLI
- [ ] Provider-specific behavior
- [ ] Installer / release packaging
- [ ] Startup / background behavior
- [ ] Documentation
- [ ] Other:

## Validation

Hosted CI runs the main frontend and Rust checks. Run the relevant local checks
before pushing and list the exact commands/results. If a check is not relevant,
say why.

- [ ] `powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1`
- [ ] For full pre-release validation: `powershell.exe -ExecutionPolicy Bypass -NoProfile -File scripts\local-check.ps1 -All -Version <version>`
- [ ] For installer/release changes: `powershell.exe -File scripts\windows-release-build.ps1 -Ref <ref> -SmokeInstall`
- [ ] Other:

## UI / tray proof

For UI, tray, settings, or visual behavior changes, attach a screenshot, short
recording, or equivalent manual proof.

- [ ] Not applicable
- [ ] Visual proof attached
- [ ] Visual proof was not practical; manual validation and explanation attached

## Notes for reviewers

Call out risky areas, follow-up work, or anything reviewers should focus on.
