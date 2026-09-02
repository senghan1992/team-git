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

- `refs/heads/<branch>` → `event = branch-push`
- `refs/tags/vX.Y.Z` → `event = release`

The shell script carries **no policy**. Which branches count as the team's
merge branch varies per project (`.gpconfig` → `merge_targets`: `main`,
`develop`, `release/1.0`, …) and can change at any time, so the classification
happens in `hook emit`, not in bash:

| pushed branch | emitted event kind | who is notified |
| --- | --- | --- |
| a merge target (`.gpconfig` `merge_targets`, else `default_base_branch`, else the repo's registered default branch) | `main_push` | **every member** — "새 병합이 반영되었습니다 / 내 브랜치에 동기화" |
| any other branch | `branch_push` | **the merge manager of the base branch only** — "브랜치가 병합을 기다립니다 / 병합하기" |

This is why a team whose merge branch is `develop` still gets correct sync
notifications: nothing is hardcoded to `main`.

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

The app re-installs the hook for every registered **local** repository on
every launch, so upgrading the binary picks up a changed template with no
manual step. The install never clobbers a repo that has its own
`core.hooksPath` pointing somewhere else.

Repositories accessed over SSH get no local hook — the working tree lives on
the remote host. For those, notifications come from the app's own merge/push
flow rather than from git.

## Removing the hook

Delete the repo entry from Git Companion's settings and unset
`core.hooksPath`:

```bash
git config --unset core.hooksPath
rm .git/hooks/pre-push
```
