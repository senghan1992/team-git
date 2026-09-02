# Product

<!-- impeccable:product-schema 1 -->

## Platform

web — the app ships as a Tauri 2 desktop shell, but the entire surface is a Vite + TypeScript web frontend with web design conventions (HTML/CSS, Tailwind v4, inline SVG icons), not OS-native UI.

## Users

- Primary user: a developer on a small-to-mid-size team who works with git every day — registers repositories (local or over SSH), switches branches, stages and commits, pushes/pulls, and resolves merge conflicts.
- Secondary user: the team lead or reviewer who acts as a per-branch **merge manager** (specified in the project's `.gpconfig`) and receives team push events in the team inbox.
- The UI language is Korean; all copy is written in Korean.

## Product Purpose

Git Companion is a lightweight desktop companion for git teams: keep a repository's state in view (branch, ahead/behind, working tree), drive the daily loop — commit, push, pull, stash — from a diff-aware UI instead of the terminal, and coordinate merges: a merge center with block-level conflict resolution (ours/theirs/manual, with an optional AI suggestion), a per-branch merge manager who locks pushes to protected branches, and a team inbox that surfaces teammates' push events by email.

## Positioning

The meaningful mechanism is **SSH auto-discovery + block-level merge governance in one companion**: repositories and remote environments are registered once (host/user/key/password/port with an SSH directory browser), then the app knows the working tree, the branch policy (who may push to which branch), and the team's push activity — so the daily git loop and the merge review loop live in one surface instead of terminal + web hooks.

## Operating Context

- Runs on a developer's desktop (Tauri window). Users work over local paths or SSH to a server where the real repository lives; remote operations go over SSH (key or password auth).
- Working view polls status every ~6 seconds (suspended while a modal is open or an input is focused) so teammates' pushes surface without manual refresh.
- Repositories are configured through a tracked `.gpconfig` project file (branch merge managers, members, channels such as Slack).
- Flow: register repo → pick working branch → work/commit/push/pull → resolve conflicts in the merge tab (block-level, ours/theirs/manual); stash for temporary parking; team inbox for push events; settings for Slack channels and account.

## Capabilities and Constraints

Confirmed functionality (from README and code):

- Repository registration: SSH host/user/key-path/password/port, connection test, SSH directory browser, branch selection, path discovery; local paths also supported.
- Working view: branch switcher (remote tracking normalized), status table (added/modified/deleted/renamed/copied/untracked/conflicted) with per-file staging checkboxes and select-all, ahead/behind pills, commit (message + stage-all or selected files), push with credential flow, pull with conflict banner, stash save/pop/drop.
- Merge tab: conflict list with block-level diff, ours/theirs/manual resolution (AI suggestion available), per-branch merge manager display; pushes to a managed branch are locked for non-managers.
- Team view: invite members by email, role admin/member, merge-manager assignment, push event inbox.
- Settings: account/session, Slack channel registration with defaults (main/branch push, release).
- Tabs per repository: 작업/병합/설정. Global nav: 저장소/팀/설정. Sidebar lists registered repos + account chip.
- Launchable from the app: an optional sub-tool (Rust sidecar) seen in the dev launcher concept (README mentions a sub-tool launcher).

Technical constraints:

- Tauri 2 app; Rust backend in `src-tauri/`; frontend uses no framework (vanilla TS modules + Tailwind v4), builds with Vite. Icons are inline Lucide-path SVGs.
- Dev-only browser mock (`_dev_shim.ts`) provides canned data for running the UI without Tauri — used for live preview.
- All copy stays Korean. Layout structure (sidebar + three views), information architecture, and every existing interaction must be preserved.
- Accessibility: keyboard focus-visible outlines, `prefers-reduced-motion` handling, semantic `dialog`/`<main>`/`button` elements already in place; preserve them.

User-confirmed constraints (redesign round):

- Light tone (light-based design), not dark.
- Must feel like a real developer tool — crafted for the working developer's daily loop, with UX decisions visible in the interface (state clarity, hierarchy, precision); "not a design that ignores user experience" is the stated rejection of the incumbent look.
- Keep current layout structure and all functionality — visual world is replaced, nothing else.

## Brand Commitments

- Product name: **Git Companion** (keep).
- UI language: Korean (keep).
- Light tone; developer-tool character; UX-first (user-confirmed this round).
- No visual references are binding; the current Airbnb-inspired look (white canvas, `#ff385c` pink accent, soft pills) is explicitly rejected and is not an asset to preserve.

## Evidence on Hand

- `README.md` — product capabilities and positioning.
- `ui/_dev_shim.ts` — realistic mock data (repos, branches, status, conflict blocks, team inbox entries, stash) usable as design material.
- `ui/` source — every view and component; `ui/components/GitGraph.ts` renders a commit graph from log data.
- No marketing/case-study material exists; do not fabricate testimonials or commercial claims.

## Product Principles

1. **State before decoration.** The user's first question is "what is my repository doing right now" — working tree, branch, ahead/behind, conflicts must be readable at a glance and never be obscured by styling.
2. **Every irreversible action is visible and confirmable.** Push, pull with conflicts, stash drop, delete — the interface states what will happen and confirms before acting.
3. **The team is in the loop.** Merge managers, push events, and member roles are first-class information, not buried settings.
4. **Precision over playfulness.** The product manages code and merges; controls behave predictably, spacing and type feel engineered, not decorated.
5. **Workflows stay fast and interruptible.** Polling, queueing, and batching keep the app responsive; the UI never blocks the user's flow.

## Accessibility & Inclusion

- Keyboard-operable controls (focus-visible outlines on buttons/links/inputs), reduced-motion support, semantic structure (`main`, `dialog`, labels), sufficient contrast for Korean text at small sizes. Contrast and focus must survive the visual redesign.