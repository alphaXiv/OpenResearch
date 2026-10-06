# Optional release highlights

To add a custom note to a release, commit `release-notes/vX.Y.Z.md` in the PR that bumps `Cargo.toml` to `X.Y.Z`. Write the Markdown exactly as it should appear above the generated changes, for example:

```md
## Highlights

The Linux desktop app now updates itself.
```

Leave the file out when there is nothing to add. The release workflow generates the PR list, linked fixed issues, comparison link, and install instructions automatically.
