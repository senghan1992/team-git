# Pre-push hooks

Git Companion installs a `pre-push` hook into every repository you register
through the app. When you `git push`, the hook fires `git-companion hook emit`
for each pushed ref; the binary POSTs the event to the team backend, which
fan-out delivers it to every project member.

## Where the hook lives

Two locations:

1. `<config_dir>/com.gitcompanion.app/hooks/pre-push` — the canonical copy
   embedded into the binary (`include_str!`-based template).
2. `<repo>/.git/hooks/pre-push` — a symlink to (1). Acts as a backup when
   `core.hooksPath` is overridden locally.

`core.hooksPath` is configured to `<config_dir>/com.gitcompanion.app/hooks`
for the registered repo so git picks it up automatically.

## What the hook does

For every ref being pushed:

- `refs/heads/main` → `event = main-push`
- `refs/heads/<other>` → `event = branch-push`
- `refs/tags/vX.Y.Z` → `event = release`

Each event calls:

```
git-companion hook emit --event <kind> \
  --author "$(git log -1 --format='%an' $local_sha)" \
  --message "$(git log -1 --format='%s' $local_sha)" \
  --sha "$(git log -1 --format='%H' $local_sha)" \
  --branch <branch> \
  --remote-url "$(git remote get-url origin)" \
  --repo <repo toplevel>
```

The `hook emit` subcommand reads `~/.config/com.gitcompanion.app/config.json`,
matches the repo by canonical path, resolves the project linked to that repo,
and POSTs the event to the team backend's fan-out endpoint.

## Push never blocks

The hook script ends with `exit 0`. If `git-companion` is not on the `PATH`,
the script silently continues the push. If the POST returns a non-2xx, the
binary logs the failure and continues the push.

## Putting `git-companion` on your `PATH`

The hook cannot find the binary unless it is installed where the shell
searches. Pick one:

- Move `target/release/git-companion` to `~/.local/bin` (or any other
  directory on `$PATH`):
  ```bash
  install -m 0755 target/release/git-companion ~/.local/bin/
  ```
- Or symlink it:
  ```bash
  ln -s "$(pwd)/target/release/git-companion" ~/.local/bin/git-companion
  ```
- Or set `GIT_COMPANION_BIN=/absolute/path/to/git-companion` in your shell
  rc.

## Reinstalling the hook

The Settings view has a **pre-push hook 다시 설치** button that walks every
registered repo and re-runs the install. Useful after upgrading the binary if
the embedded template changed.

## Removing the hook

Delete the repo entry from Git Companion's settings and unset
`core.hooksPath`:

```bash
git config --unset core.hooksPath
rm .git/hooks/pre-push
```
