---
name: herdr-fork-sync
description: Update this herdr fork (JacobDeuchert/herdr, branch master) to upstream herdrdev/herdr. Merges upstream in, checks each patch's removal condition, repairs broken patches and adapted tests, and updates FORK.md. Use when the user says "update to upstream", "sync with upstream", "merge upstream", "pull upstream", or names a new upstream herdr version.
---

# Herdr fork upstream sync

`FORK.md` is the source of truth. It lists every fork patch with its anchor,
patch text, removal condition, verification command, and adapted upstream tests.
Read all of it before starting. This skill is the procedure. FORK.md is the
data. If this skill and FORK.md disagree, follow FORK.md and fix this skill.

The user expects the whole procedure to run without being prompted step by step.
Stop only at the checkpoints marked **ask**.

## 1. Preflight

- Run this in the main checkout (`~/src/github/herdr`) on branch `master`.
  The fork's `master` carries the patches; it is never a plain mirror of
  upstream. The working tree must be clean. If it isn't, stop and report.
- `git fetch upstream --tags --prune` and `git fetch origin`.
- Choose the target:
  - Default: `upstream/master`. FORK.md records the base as
    `<sha> (post vX.Y.Z)`.
  - If the user names a version, use that tag. Stable tags sit on release
    commits that are **not ancestors of upstream master**. Merging one brings in
    its version-bump commit, so a later merge of upstream master may conflict on
    the version in `Cargo.toml`/`Cargo.lock`. Take upstream's side there.
  - Get the version label with
    `git tag --sort=-creatordate | grep '^v' | head -1`.
- Record the old base: `git merge-base HEAD upstream/master`.
- Check the fork's own changes with
  `git log --oneline --no-merges <old-base>..HEAD` and
  `git diff --stat <old-base> HEAD`. Every changed file must belong to a
  FORK.md patch, FORK.md itself, or `.claude/skills/herdr-fork-sync/`.
  Anything else is an untracked divergence. Flag it to the user.
- No backup branch is needed. A merge doesn't rewrite history, and
  `git reset --hard ORIG_HEAD` undoes it before pushing.

## 2. Check each patch's removal condition before merging

For every `status: active` entry in FORK.md, check its `remove when:` condition
against the target. Use evidence, not commit subjects alone:

- `git log --oneline <old-base>..<target> -- <local files of the patch>` shows
  what upstream changed in the patch's files.
- Read upstream's current version of the anchor, for example
  `git show <target>:src/client/shell/copy_mode.rs`. Check whether upstream
  now handles the behavior natively.
- Open upstream discussions and issues named in FORK.md (for example
  `gh api repos/herdrdev/herdr/discussions/587 --jq '.state,.title'`, or
  `gh issue list -R herdrdev/herdr --search ...`). Also search upstream config
  docs and code for a new config surface, such as `copy_mode_` keys in
  `src/config/`.
- Search for the patch's behavior directly. For example, for F13-F24 check
  `encode_f_key` in upstream's `src/input/encode.rs`.

Classify each patch: **keep**, **remove** (upstream covers it fully), or
**replace with config** (upstream added a config surface). If the result is
remove or replace, and the patch's behavior would change in a way the user
would notice, **ask** before deleting it. Show the evidence. If upstream covers
it exactly, go ahead and remove it.

## 3. Merge

```sh
git merge --no-ff <target> -m "chore(fork): merge upstream <label>"
```

`<label>` is the newest version plus the short SHA when merging master, like
`v0.9.5 (ee5bfb50)`, or just the tag when merging a tag.

For each conflict, follow FORK.md: **don't resolve hunk by hunk**. Take
upstream's version of the file whole with `git checkout --theirs <file>`.
During a merge, `--ours` is the fork's `master` and `--theirs` is upstream.
Then reapply the patch by hand from its FORK.md entry at the described anchor,
`git add` the file, and finish with `git commit --no-edit`. If the anchor has
moved or been renamed, find its new home by function name and describe it in
FORK.md.

To remove a patch, undo it in its own commit after the merge
(`revert(fork): drop patch 000N`). `git revert` of the original patch commit
usually won't apply after upstream drift, so undo it by hand against the
current code. Then delete its FORK.md entry and add a line under a
"Retired patches" section with the upstream commit or version that replaced
it.

The whole fork patch set is always `git diff upstream/master...master`
(three dots: changes on the fork side since the merge base).

## 4. Verify and repair

Run each patch's `verification:` command from FORK.md, then the full suite:

```sh
just check
```

Toolchain: `cargo`, `just`, `cargo-nextest`, and Zig aren't on the host
PATH. Run every build or test command through the flake dev shell, as in
`nix develop --command just check`. A cold build takes well over 10 minutes,
so run it in the background.

Debug build note: `just check` needs Zig 0.16.0 for vendored libghostty-vt and
the Windows cross SDK for its Windows stage. If the Windows SDK isn't set up,
run `just test` plus `cargo fmt --check` and report the narrower validation.

Known host baseline (NixOS dev box, as of the v0.9.5 sync): about 60 tests
fail on **pristine upstream** too. They are machine_api, machine_setup,
remote_attach, worktrees, api_ping cwd, layouts, headless, and wl-copy tests,
where fake `ssh` scripts can't find `cat`. `maintenance-test` also needs
`openssl`. `ghostty-vt
deep_scrollback_resize_preserves_unicode_and_hyperlinks` runs for 40+ minutes
because the dev shell builds libghostty in Debug, so exclude it with
`-E 'not test(deep_scrollback_resize_preserves_unicode_and_hyperlinks)'`.
Always run with `--no-fail-fast`. To compare against upstream, run only the
failing set on a clean worktree with `CARGO_TARGET_DIR` pointing at the main
checkout's `target/`, so only herdr recompiles.

Classify failures:

1. **Upstream test using a remapped key.** Patch 0002 remaps copy-mode keys. New
   or changed upstream copy-mode tests that press `h j k l w b n N 0 $ / q` (or
   others in the FORK.md remap table) fail because those keys now do something
   else. Fix by moving the press to the fork key with a `// fork:` comment.
   Don't change assertions. Add every such test to the "adapted upstream tests"
   table in FORK.md, and remove rows for tests upstream deleted or renamed.
   Check with `grep -n 'fork:' src/client/shell/tests/*.rs`.
2. **Patch broken by upstream refactor.** Reapply at the new anchor. Keep the
   design constraints FORK.md states (for example, don't remap inside
   `copy_mode_command_char`, and keep the `return` in the ctrl+c arm).
3. **Unrelated upstream failure.** Check by running the same test on a clean
   `<target>` worktree (`git worktree add ~/worktrees/herdr/upstream-check <target>`).
   If it fails upstream too, report it and don't fix it in the fork.
   Remove the worktree afterwards.

Also scan the upstream range for new copy-mode motions or keys
(`git log <old-base>..<target> -- src/client/shell/copy_mode.rs src/copy_mode*`).
New upstream motions arrive on their default key, which may now collide with
or be shadowed by the remap. List them for the user.

## 5. Update FORK.md and commit

- Update every patch's `upstream base:` line to the new base, in the form
  `` `<short-sha>` (post `vX.Y.Z`) `` (or the tag itself when merging a
  tag).
- Update anchors, local files, the adapted-tests table, the "Why these are
  patches" section (version number and discussion status), and the
  "Not patched" notes if upstream changed any of them.
- Commit with the `commit-by-convention` skill:
  `docs(fork): sync patches with upstream <label>`. The body says which
  patches were kept, repaired, or removed, what moved, and which upstream
  tests were adapted. Don't add AI co-author lines (project rule).
- Patch-code repairs and adapted tests that weren't resolved inside the merge
  commit go in their own commits after the merge, like
  `fix(fork): reapply colemak remap after <upstream change>` or
  `test(fork): move <test> onto fork keys`. Never rewrite published history.

## 6. Finish

- Show a summary. For each patch, give kept/removed/repaired, the upstream
  version, adapted tests, new unmapped upstream copy-mode keys, and validation
  results.
- **ask** before pushing: `git push origin master`. This is a normal
  fast-forward push; never force-push `master`.
- Don't install the binary. The installed `herdr` comes from the user's Nix
  home-manager config, so tell the user it needs a rebuild.
- Mention leftover `fork/copy-mode*` branches from the old rebase workflow the
  user may want to delete. Don't delete them without asking.
