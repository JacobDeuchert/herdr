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

The build needs Zig 0.15.2 for the vendored `libghostty-vt`. Fork builds do not
receive `herdr update`; reinstall the binary yourself.

## Why these are patches and not config

Upstream herdr 0.8.0 makes only the key that *enters* copy mode configurable
(`keys.copy_mode`). Every key *inside* copy mode is hardcoded. Making them
configurable is an open upstream request with no implementation:
https://github.com/herdrdev/herdr/discussions/587

If that discussion ships a `copy_mode_*` config surface, all patches below
become deletable in favor of config in `~/.config/herdr/config.toml`.

---

## 0001 ctrl+c exits copy mode

status: active

upstream base: `9e6c2b4e` (post `v0.8.0`)

local files:

- `src/app/input/copy_mode.rs`

anchor: `AppState::handle_copy_mode_key`, the `match (key.code, key.modifiers)`
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
(KeyCode::Char('c'), mods) if mods.contains(KeyModifiers::CONTROL) => {
    self.exit_copy_mode(terminal_runtimes, false);
    return;
}
```

The `return` matters. The other arms in this block deliberately fall through to
the character dispatch below, which is harmless for them because
`copy_mode_command_char` rejects ctrl chords. Exiting first and then falling
through would run the character dispatch against a torn-down `copy_mode` state.

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

upstream base: `9e6c2b4e` (post `v0.8.0`)

local files:

- `src/app/input/copy_mode.rs`

anchor: `AppState::handle_copy_mode_key`, immediately after

```rust
let Some(ch) = copy_mode_command_char(key) else {
    return;
};
```

and immediately before upstream's `match ch { ... }` character dispatch.

reason: Ports the `copy-mode-vi` table from the pre-herdr tmux config (Colemak,
`neio` as the arrow cluster) onto herdr's fixed copy-mode keymap.

patch shape: a remap expression that rebinds `ch` before upstream's dispatch
runs, plus a small `AppState::fork_repeat` helper and a `FORK_JUMP` constant.
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

The remap must **not** move into `copy_mode_command_char`. The search-prompt
handler calls that same function to build the query string, so remapping there
would corrupt what you type into a `/` search.

conflict risk: low. The insert point is a stable two-line anchor and upstream's
arms are untouched. If upstream adds a motion, it arrives unmapped on its
default key; decide then whether to fold it into the table.

remove when: upstream ships configurable copy-mode keys per discussion #587, at
which point this becomes `copy_mode_*` entries in `config.toml`.

verification:

```sh
cargo nextest run --locked copy_mode_colemak
```

---

## 0003 encode F13-F24 for child terminals

status: active

upstream base: `9e6c2b4e` (post `v0.8.0`)

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
