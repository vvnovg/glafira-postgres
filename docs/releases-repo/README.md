# Glafira for PostgreSQL — releases

Desktop app for diagnosing PostgreSQL servers and Patroni clusters. Download the installer for your OS from [Releases](../../releases/latest).

User guide: [English](guide/en/getting-started.md) · [Русский](guide/ru/getting-started.md)

## First launch
**macOS** — the `.dmg` build is signed with an Apple Developer ID and notarized: the app opens without extra steps.
The `.pkg` installer is meant for managed deployment (MDM); it is signed when an installer certificate is configured
and is not notarized separately, so Gatekeeper may warn if you run it by hand.

**Windows** — the installer is not signed yet, so SmartScreen warns on first launch: click "More info" → "Run anyway",
or unblock the installer in PowerShell:
```powershell
Unblock-File -Path "$env:USERPROFILE\Downloads\Glafira_for_PostgreSQL_*_x64_en-US.msi"
```
Updates installed from inside the app don't need this.

## Source code and support
Glafira is free and open source under the Apache License 2.0: https://github.com/vvnovg/glafira-postgres.
All features are available to everyone. If Glafira is useful to you, see [SUPPORT.md](SUPPORT.md).

PostgreSQL is a registered trademark of the PostgreSQL Community Association of Canada. This project is not affiliated with it.
