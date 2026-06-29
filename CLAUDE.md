# SpoitableHRS

## Versioning

Date-based semver: `YYYY.M.BUILD`

- `YYYY` — year
- `M` — month (no leading zero)
- `BUILD` — build count within the month, starting from 0. Resets to 0 each month.

Examples: `2026.6.0`, `2026.6.1`, `2026.7.0`

## Version bump checklist

When releasing a new version, update **both** of these files:

1. `src-tauri/tauri.conf.json` — `"version"` field
2. `package.json` — `"version"` field

Both must have the same version string. The git tag must match with a `v` prefix (e.g. `v2026.6.1`).

Do NOT use PowerShell `Set-Content` to edit these files — it adds a UTF-8 BOM which breaks the Tauri build. Use `sed` or the Edit tool instead.
