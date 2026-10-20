# SSH troubleshooting

## An installed `orx` is rejected because of permissions

The remote binary probe requires the resolved executable to be owned by the
SSH user or root. Neither that executable nor its immediate parent directory
may be writable by group or others. These checks also apply after installation
and after resolving a symlink.

For example, an executable with mode `755` in a directory with mode `775` is
rejected even if `orx --version` works when run manually. Reinstalling into the
same directory does not necessarily change its permissions. The explicit-path
probe reports this permission rejection rather than treating it as a missing
installation.

Inspect the resolved executable path reported in the error and its parent on
the remote host. For a default installation without symlinks:

```sh
ls -ld "$HOME/.cargo/bin" "$HOME/.cargo/bin/orx"
```

Only if these paths are intended to be private to your account, remove group
and other write permission from the affected paths, then reconnect:

```sh
chmod go-w "$HOME/.cargo/bin" "$HOME/.cargo/bin/orx"
```

This preserves owner write permission and existing read/execute permissions.
For a custom installation or symlink, inspect and use the resolved paths from
the error instead. Do not recursively change permissions or change a shared
tool directory without its administrator's agreement. Use a private install
location if group write access is intentional. The app does not change these
permissions automatically or exempt the user's primary group from the check.
