# Fork-local patches

This fork tracks `herdrdev/herdr` and carries a small set of deliberate local
changes. This file is the complete inventory. Nothing else in this tree is
intentionally divergent from upstream.

The point of this file: when a rebase conflicts, **take upstream's side wholesale**
and reapply the patches below from scratch. Every patch is written to be
reapplied by hand in a few minutes, against an anchor that is described in prose
rather than by line number, so upstream drift does not invalidate it.

## Remotes

```
origin    https://github.com/JacobDeuchert/herdr   (this fork)
upstream  https://github.com/herdrdev/herdr        (canonical)
```

## Upgrade procedure

```sh
git fetch upstream
git rebase upstream/master          # or: git rebase <tag>
```

If a patch conflicts, do not resolve it hunk by hunk:

```sh
git checkout --theirs <file>        # take upstream's version whole
```

Then reapply the patch from its entry below, run its verification, and update
the entry's `upstream base` line. If upstream has implemented the behavior
natively, delete the patch and its entry instead — each entry states the exact
condition for that.

Rebuild after any upgrade:

```sh
just check
cargo build --release
```

The build needs Zig 0.16.0 for the vendored `libghostty-vt`. Fork builds do not
receive `herdr update`; reinstall the binary yourself.

## Why these are patches and not config

Upstream herdr 0.9.0 makes only the key that *enters* copy mode configurable
(`keys.copy_mode`). Every key *inside* copy mode is hardcoded. Making them
configurable is an open upstream request with no implementation:
https://github.com/herdrdev/herdr/discussions/587

If that discussion ships a `copy_mode_*` config surface, all patches below
become deletable in favor of config in `~/.config/herdr/config.toml`.

---

## 0001 ctrl+c exits copy mode

status: active

upstream base: `fff6c820` (post `v0.9.1`)

local files:

- `src/client/shell/copy_mode.rs`
- `src/client/shell/tests/copy.rs`

anchor: `ClientShellState::route_copy_mode_key`, the `match (key.code, key.modifiers)`
block that handles the `ctrl+b` / `ctrl+f` / `ctrl+u` / `ctrl+d` page motions.

reason: The Colemak remap in patch 0002 moves `q` to start-of-line, which
removes upstream's non-`Esc` exit. `ctrl+c` is unclaimed in copy mode: the ctrl
block handles only `b/f/u/d`, and `copy_mode_command_char` rejects any modifier
other than Shift, so `ctrl+c` is currently a silent no-op. Copy mode consumes
every key with no PTY forwarding path, so this cannot reach the pane as SIGINT.

patch: add as the first arm of that match, before the `ctrl+b` arm:

```rust
// fork: ctrl+c leaves copy mode without copying. Copy mode consumes
// every key, so this never reaches the pane as SIGINT.
(KeyCode::Char('c'), modifiers) if modifiers.contains(KeyModifiers::CONTROL) => {
    self.exit_copy_mode(false, outcome);
    outcome.repaint = true;
    return;
}
```

The `return` matters. The other arms in this block deliberately fall through to
the character dispatch below, which is harmless for them because
`copy_mode_command_char` rejects ctrl chords. Exiting first and then falling
through would run the character dispatch against a torn-down `copy_mode` state.
The explicit `outcome.repaint = true` matters for the same reason: the shared
repaint at the end of the function is what the returning arms skip.

conflict risk: low. Purely additive, in a block upstream changes rarely.

remove when: upstream binds `ctrl+c` in copy mode, or ships configurable
copy-mode keys per discussion #587.

verification:

```sh
cargo nextest run --locked copy_mode_ctrl_c_exits_without_copying
```

---

## 0002 colemak copy-mode keymap

status: active

upstream base: `fff6c820` (post `v0.9.1`)

local files:

- `src/client/shell/copy_mode.rs`
- `src/client/shell/tests/copy.rs`
- `src/client/shell/tests/input.rs`

anchor: `ClientShellState::route_copy_mode_key`, immediately after

```rust
let Some(command) = crate::copy_mode::copy_mode_command_char(key.clone()) else {
    return;
};
```

and immediately before upstream's `match command { ... }` character dispatch.

reason: Ports the `copy-mode-vi` table from the pre-herdr tmux config (Colemak,
`neio` as the arrow cluster) onto herdr's fixed copy-mode keymap.

patch shape: a remap expression that rebinds `command` before upstream's dispatch
runs, plus a small `ClientShellState::fork_repeat` helper and a `FORK_JUMP`
constant.
**Upstream's match arms are not edited**, so new upstream motions land without
conflicting. The six repeat-5 keys have no upstream equivalent and are executed
in the remap block itself rather than dispatched.

| key | action | upstream key it borrows |
| --- | --- | --- |
| `n` `e` `i` `o` | move left / down / up / right | `h` `j` `k` `l` |
| `N` `E` `I` `O` | same, jump `FORK_JUMP` (5) | none — run in the remap block |
| `q` | start of line | `0` |
| `p` | end of line | `$` |
| `w` / `W` | previous word / ×5 | `b` |
| `f` / `F` | next word / ×5 | `w` |
| `!` | search forward | `/` |
| `k` / `K` | next / previous match | `n` / `N` |

Unremapped upstream keys still work and are deliberately left alone: `?`, `v`,
Space, `V` (linewise select), `y`, Enter, `g`/`G`, `{`/`}`, `^`, `0`, `$`, `/`,
`h`/`j`/`l`, `ctrl+b/f/u/d`, PageUp/PageDown, arrows, Home/End, Esc. Note `q` no
longer exits — that is what patch 0001 is for.

Shadowed upstream keys: `W`, `E`, `B`, `N` are upstream's big-word and
reverse-search keys, and the remap takes `W`, `E`, `N` for jump-5 motions.
`B` still reaches upstream's previous-big-word. Reverse search stays available
on `K`.

Since `v0.9.0` the word, paragraph, and line-end motions are endpoint-backed
(`request_copy_motion` → `PaneCopyMotion`) rather than local cursor math, so the
`W`/`F` repeats enqueue `FORK_JUMP` operations on `copy_operation_queue` and the
client drains them one round trip at a time. Cursor motions (`N`/`E`/`I`/`O`)
are still local and apply immediately. The repeat helper therefore sets
`outcome.repaint` itself instead of falling through to the dispatch's repaint.

The remap must **not** move into `copy_mode_command_char`. The search-prompt
handler calls that same function to build the query string, so remapping there
would corrupt what you type into a `/` search.

adapted upstream tests: the remap changes which key drives a behavior, so six
key presses in upstream's own tests had to move onto their fork equivalents.
Each is marked with a `// fork:` comment. Nothing about the assertions changed.

| test | upstream key | fork key |
| --- | --- | --- |
| `keyboard_selections_survive_output_and_copy_live_ranges` | `k` | `i` |
| `keyboard_selection_does_not_return_after_resize_or_screen_switch` | `vk` | `vi` |
| `keyboard_copy_mode_content_motion_is_endpoint_backed_and_stale_safe` | `w` | `f` |
| `copy_search_owns_prompt_repeat_highlights_selection_and_restore` | `n`, `N` | `k`, `K` |
| `copy_mode_repeat_during_projection_gap_stays_active` | `k` (×2) | `i` |
| `highlighted_search_match_copies_after_in_flight_repeat` | `n` | `k` |

The first five are in `src/client/shell/tests/copy.rs`; the last is in
`src/client/shell/tests/input.rs`.

When a rebase brings new upstream copy-mode tests, expect the same treatment:
grep the failures for these keys before assuming the remap itself regressed.

conflict risk: low for the remap block itself — the insert point is a stable
two-line anchor and upstream's arms are untouched. Medium for the adapted tests
above, which sit in a file upstream edits often. If upstream adds a motion, it
arrives unmapped on its default key; decide then whether to fold it into the
table.

remove when: upstream ships configurable copy-mode keys per discussion #587, at
which point this becomes `copy_mode_*` entries in `config.toml`.

verification:

```sh
cargo nextest run --locked copy_mode_colemak
```

---

## 0003 encode F13-F24 for child terminals

status: active

upstream base: `fff6c820` (post `v0.9.1`)

local files:

- `src/input/encode.rs`

anchor: `encode_f_key`, after the existing `F12` arm.

reason: Herdr receives extended function keys from crossterm, but its child
terminal encoder only handles `F1` through `F12`. `F13` therefore produces no
PTY input. Encode `F13` through `F24` using xterm's standard shifted `F1`
through `F12` sequences.

conflict risk: low. One additive match arm and a focused encoder test.

remove when: upstream's function-key encoder supports `F13` through `F24`.

verification:

```sh
cargo test --locked legacy_f13_to_f24_use_shifted_function_key_sequences
```

---

## Not patched, deliberately

Things the tmux config used to do that this fork does *not* try to restore:

- **Rectangle / block selection** (tmux `V` = rectangle-toggle). Upstream PR
  #2352 was closed unmerged. In herdr `V` is linewise select.
- **Count prefixes** (tmux `5j`). Upstream has no count support. Patch 0002 may
  add fixed jump-5 keys instead, which is not the same thing.
- **`copy-pipe` to `wl-copy`.** herdr owns its clipboard path and emits OSC 52,
  which is the wanted behavior over SSH anyway.
- **vim-aware pane navigation** (tmux `if-shell "$is_vim"`). No upstream
  equivalent and out of scope for a keymap fork.
- **Direct chords inside copy mode.** Upstream closed #2242 as
  expected-behavior, so `alt+n/e/i/o` stay inert in copy mode. Changing this
  means touching input routing, not the copy-mode keymap, and is a much larger
  patch than this fork wants to carry.

## Configuration, not forked

Most of the tmux port needed no patch at all and lives in
`~/.config/herdr/config.toml` — prefix, splits, pane focus/swap, tabs, agents,
session bindings, and incremental pane resize via `resize_pane_left/down/up/right`.
Keep customization there whenever it is expressible; this file is only for what
config cannot reach.
