# Product

<!-- impeccable:product-schema 1 -->

## Platform

web — the app ships as a Tauri 2 desktop shell, but the entire surface is a Vite + TypeScript web frontend with web design conventions (HTML/CSS, Tailwind v4, inline SVG icons), not OS-native UI.

## Users

- Primary user: a developer on a small-to-mid-size team who works with git every day — registers repositories (local or over SSH), switches branches, stages and commits, pushes/pulls, and resolves merge conflicts.
- Secondary user: the team lead or reviewer who acts as a per-branch **merge manager** (specified in the project's `.gpconfig`) and receives team push events in the team inbox.
- The UI language is Korean; all copy is written in Korean.

## Product Purpose

Git Companion is a lightweight desktop companion for git teams working one project through per-person branches. It closes a single loop: each member cuts their own branch, commits, and pushes to their own remote branch; the designated **merge manager** is notified, reviews what is changing, merges into the team's merge branch, and pushes; every member is then notified and syncs the merged code back into their branch. Around that loop it keeps repository state in view (branch, ahead/behind, working tree), suggests the one next action per repository, shows a file-centric **change map** of what every waiting branch is touching, and repairs merge conflicts with a pre-configured AI resolver.

## Positioning

The meaningful mechanism is **one role-aware loop with the merge step automated but never destructive**: the repository, the branch policy (`.gpconfig`), and the team's push activity are known in one place, so each person is shown the single next thing they owe the team, the manager sees a file-level map of what AI agents are changing across branches before merging, and a crashed merge repairs itself from a prompt written in advance — stopping and handing back rather than discarding a teammate's commits when it cannot. SSH auto-discovery makes the same loop work against repositories that live on a server.

## Operating Context

- Runs on a developer's desktop (Tauri window). Users work over local paths or SSH to a server where the real repository lives; remote operations go over SSH (key or password auth).
- Working view polls status every ~6 seconds (suspended while a modal is open or an input is focused) so teammates' pushes surface without manual refresh.
- Team rules are configured through a tracked `.gpconfig` project file committed into the repository itself (merge target branches, per-branch merge managers, members) — read from the merge branch when the checked-out branch has no copy, so a member on their own branch still sees the team's rules.
- Flow: register repo → set `.gpconfig` (merge branch + merge manager) → member cuts a branch, works, commits, pushes → manager is notified, reads the change map, merges (AI repairs conflicts) and pushes → members are notified and sync. Stash for temporary parking; team inbox for push events; settings for AI auto-merge, SSH, push credentials, and account. Documented end to end in `docs/WORKFLOW.md`.

## Capabilities and Constraints

Confirmed functionality (from README and code):

- **Login is optional.** Everything git-side (register a repo, commit, push, pull, merge, resolve conflicts, AI auto-merge, `.gpconfig`) works signed out; only push notifications and member lookup need an account. The first launch shows a one-time welcome that says exactly which is which and offers "저장소 열고 시작하기". Locking the app behind login had meant a first-time user could not open the app at all without self-hosting the FastAPI server.
- Login accounts on the team server (SQLite `users` table): register, sign in by id **or** email, profile edit, password change, sign out, delete account, and teammate lookup for adding `.gpconfig` members. Passwords are PBKDF2-HMAC-SHA256 with a per-row salt; the app caches only the signed-in user + a revocable token, so it stays signed in offline but cannot mint a new login without the server. My-page (sidebar name) follows the familiar shape: profile header → 프로필 → 비밀번호 변경 → 로그아웃 → collapsed 회원 탈퇴.
- Repository registration: SSH host/user/key-path/password/port, connection test, SSH directory browser, branch selection, path discovery; local paths also supported.
- Home: one card per repository showing branch, state pills (충돌 / 변경 / ↑미푸시 / ↓뒤처짐 / 병합 대기), and a single **다음 할 일** derived from state + the viewer's role — resolve → merge → commit → push → sync, in that priority. Sync runs inline from the card; everything else routes to the right tab. The rule is a pure, unit-tested function (`ui/components/nextAction.ts`).
- Working view: branch switcher (remote tracking normalized), new-branch creation (+ optional immediate push), sync-from-base, status table (added/modified/deleted/renamed/copied/untracked/conflicted) with per-file staging checkboxes and select-all, per-file diff preview, ahead/behind pills, commit (message + stage-all or selected files), push with credential flow, pull with conflict banner, stash save/pop/drop.
- Merge tab: **변경 지도** (file → branches/authors; files touched by two or more branches pinned on top with a warning, rest collapsed), pending-branch list with ahead/behind, changed files, collapsible commits and 20 s auto-detect, one merge-manager badge for the base branch, block-level conflict resolution (ours/theirs/manual, per-block AI suggestion, undo), one-click AI auto-merge, pre-resolution backup restore, post-merge push banner. Merges and pushes to a managed branch are locked for non-managers.
- AI auto-merge (Settings): enable + endpoint/model/key, a **pre-configured resolver prompt** reused for every conflicted file, "충돌이 나면 곧바로 자동 해결" so a conflicted merge repairs itself with no click, and a side strategy for binary/oversized files. Safety rules: originals are always backed up first; a file still containing conflict markers is never committed; and when the AI output is unusable for a text file both sides changed, the resolver deliberately leaves it for a person instead of discarding one side's commits.
- Notifications view: received events, plus the delivery config (server URL, team + join code, which repositories notify, and recipients synced from `.gpconfig` with one button). Desktop notifications are role-aware: a push to the merge branch notifies every member ("내 브랜치에 동기화"); a push to a work branch notifies only that branch's merge manager ("병합하기").
- Per-repo 설정 tab writes `.gpconfig` into the repository (merge target branches, per-branch merge managers, members), so every collaborator sees the same rules with no server. The merge branch need not be `main`, and push-notification classification follows the same config.
- Tabs per repository: 작업/병합/설정. Global nav: 저장소/알림/설정. Sidebar lists registered repos + account chip.

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
- `ui/_dev_shim.ts` — realistic mock data (repos, branches, status, conflict blocks, pending branches, AI config, team inbox entries, stash) usable as design material.
- `dev/git-bridge.ts` — Vite dev-server plugin answering the real IPC surface against real git, so the browser preview behaves like the shipped app.
- `docs/WORKFLOW.md` — the team loop mapped screen by screen; the source of truth for what each role sees.
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