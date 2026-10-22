# Updates and diagnostics

This chapter covers how to update the app, the [[ui:sidebar.about]] screen, the diagnostics archive and how to support the project.

## Updates {#updates}

When it starts, the app checks once whether a new version is available. If there is one, a notice above the tabs gives its version. [[ui:about.notice.more]] opens the [[ui:sidebar.about]] screen. If the check fails, for example without internet access, no message is shown.

To check manually:

1. Open [[ui:sidebar.about]] at the bottom of the left panel.
2. In [[ui:about.updates.title]], click [[ui:about.updates.check]].
3. If a new version is available, the screen shows its number and release notes. Click [[ui:about.updates.install]].

The app downloads the update, checks its signature, installs it and restarts. Open connections are closed before the restart; connection profiles, settings and other data are kept. A download interrupted by a network problem is retried automatically, up to three attempts in total. If all three attempts fail, the app reports [[ui:errors.updates.downloadInterrupted]]. If the signature check fails, the app reports [[ui:errors.updates.badSignature]].

Installing may ask for an administrator password, because the app is installed for all users of the computer: by the `.pkg` installer on macOS, the `.msi` installer on Windows, or a `.deb` or `.rpm` package on Linux. Users without administrator rights cannot update such an installation.

Updates are downloaded from the releases page on GitHub. If the check fails with [[ui:errors.updates.checkFailed]], check your network or proxy. [[ui:errors.updates.notConfigured]] means that this build was made without the updater key, for example a development or test build. In that case, download new versions from the releases page: https://github.com/vvnovg/glafira-postgres-releases/releases

An organization can turn off update checks through a policy, or point them to an internal mirror. When checks are turned off, the [[ui:about.updates.title]] section says [[ui:errors.updates.managed]] after you click [[ui:about.updates.check]]. Like [[ui:errors.updates.notConfigured]], this message appears only after the click; the check at startup shows nothing in these cases.

## About {#about}

Open [[ui:sidebar.about]] at the bottom of the left panel. On macOS, it is also in the Help menu as [[ui:menu.about]].

The screen shows a short description of the app and:

- [[ui:about.version]] — the version number, and [[ui:about.build]] — the identifier of the source code it was built from. Include both in a support request.
- [[ui:about.system]] — the operating system and processor architecture.
- [[ui:about.license]] — Apache 2.0; the link opens the license text in the browser.
- [[ui:about.policy]] — [[ui:about.policySet]] or [[ui:about.policyNone]]. Errors in the policy are listed below.
- [[ui:about.dataDir]] — the folder with connection profiles and settings.
- [[ui:about.logDir]] — the folder with log files.

The [[ui:about.updates.title]] and [[ui:about.diagnostics.title]] sections are also on this screen.

## Diagnostics archive {#diagnostics}

The diagnostics archive collects what support needs to understand a problem. To create it:

1. Open [[ui:sidebar.about]].
2. In [[ui:about.diagnostics.title]], click [[ui:about.diagnostics.collect]]. On macOS, you can also choose [[ui:menu.diagnostics]] in the Help menu.
3. Choose where to save the file. The suggested name is `diagnostics-<date>.zip`.

When the archive is saved, the screen shows its path. The ZIP file contains:

- `info.json` — the app version and build, the operating system, the data and log folder paths and the policy status;
- `connections.json` — connection profiles: names, environments, connection kinds, node and Patroni REST API addresses, user names, databases, certificate file paths and member address overrides;
- `logs/` — the app log files.

The archive contains no passwords: they are kept in the system secret store, and passwords inside Patroni API addresses are removed. Certificate files are not included either. Host names, user names and file paths are stored as plain text, and the logs contain node addresses from PostgreSQL and Patroni error messages. Look through the archive before you send it outside your company.

The app sends nothing by itself. Attach the archive to your support request yourself.

## Support the project {#support}

Glafira is free and open source under the Apache License 2.0. Donations keep it going. [[ui:support.title]] at the bottom of the left panel, on the [[ui:sidebar.about]] screen and, on macOS, in the Help menu opens the support page in the browser: https://github.com/vvnovg/glafira-postgres-releases/blob/main/SUPPORT.md
