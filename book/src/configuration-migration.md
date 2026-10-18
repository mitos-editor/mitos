# Configuration migration

Mitos accepts known older setting names without rewriting your configuration.
To update a file to the current names, run:

- `:config-migrate` for your user `config.toml`, or the file selected with `--config`.
- `:config-migrate-workspace` for the trusted project's `.mitos/config.toml`.

The dialog names the file and asks you to choose **Migrate**. **Cancel** is
selected initially; `Esc` also cancels. Save or discard unsaved edits to an
open configuration buffer before migrating.

## What changes

Migration preserves values, comments, keybindings, and custom commands. It
updates configuration keys and creates an adjacent `config.toml.bak` backup.
Existing backups are kept with numbered suffixes, such as `config.toml.bak.1`.
For a symlink, the target is updated and the backup is placed beside the target.
The dialog shows the target, and the result reports the backup path.

Already migrated files are left unchanged, and missing files are not created.
Read-only files are not rewritten. If validation, backup, or rewriting fails,
you can keep using the older names. `languages.toml` is unchanged.

Project migration changes the project's working tree. An untrusted, excluded,
or changed workspace must be reviewed and trusted before migration. Migrating
an already trusted file preserves its grant only when all project configuration
changes come from that migration and its backup. Other changes require renewed
[workspace trust](./workspace-trust.md).

## Configuration managed by another tool

For generated or read-only configuration, export the updated TOML:

```sh
ms --migrate-config ~/.config/mitos/config.toml --stdout > /tmp/mitos-config.toml
```

This reads only the specified file and creates no backups. Inspect the output
and update the source managed by Home Manager or your dotfile manager.
Write the output to a different file: redirecting it to the input file would
truncate your configuration.
