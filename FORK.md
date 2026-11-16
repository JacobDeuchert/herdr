# Fork-local patches

This fork tracks `herdrdev/herdr` and carries a small set of deliberate local
changes. This file is the complete inventory. Nothing else in this tree is
intentionally divergent from upstream.

The point of this file: when an upstream merge conflicts, **take upstream's side wholesale**
and reapply the patches below from scratch. Every patch is written to be
reapplied by hand in a few minutes, against an anchor that is described in prose
rather than by line number, so upstream drift does not invalidate it.

## Remotes

```
origin    https://github.com/JacobDeuchert/herdr   (this fork)
upstream  https://github.com/herdrdev/herdr        (canonical)
```

The fork's `master` carries the patches. Upstream is merged into it, never
rebased under it, so `master` is only ever fast-forward pushed. The full patch
set at any time is `git diff upstream/master...master`.

## Upgrade procedure

```sh
git fetch upstream
git merge --no-ff upstream/master   # or: git merge --no-ff <tag>
```

If a patch conflicts, do not resolve it hunk by hunk:

```sh
git checkout --theirs <file>        # mid-merge, "theirs" is upstream: take it whole
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

The `herdr-fork-sync` skill in `.claude/skills/` automates this whole
procedure. It is fork-only and has no upstream counterpart.

## Why these are patches and not config

Upstream herdr 0.9.5 makes only the key that *enters* copy mode configurable
(`keys.copy_mode`). Every key *inside* copy mode is hardcoded. Making them
configurable is an open upstream request with no implementation:
https://github.com/herdrdev/herdr/discussions/587

If that discussion ships a `copy_mode_*` config surface, patches 0001 and 0002
become deletable in favor of config in `~/.config/herdr/config.toml`.

Patch 0004 is a feature rather than a keymap: plugin v1 has no sidebar UI
("native non-terminal plugin UI is not part of plugin v1", plugins docs), so a
pinned row cannot come from a plugin.

Patch 0005 is a feature for the same reason, and it also needs server facts
plugins cannot add to the client-shell projection (the pane's resume command,
a per-workspace history of them) plus one new client-shell method.

---

## 0001 ctrl+c exits copy mode

status: active

upstream base: `ee5bfb50` (post `v0.9.5`)

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

upstream base: `ee5bfb50` (post `v0.9.5`)

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

When a merge brings new upstream copy-mode tests, expect the same treatment:
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

## 0003 encode F13-F24 as shifted F1-F12

status: active

upstream base: `ee5bfb50` (post `v0.9.5`)

local files:

- `src/pane/input.rs`
- `src/pane/terminal.rs` (test only)

anchor: `legacy_shell_key`, the exception table for panes that negotiated no
keyboard protocol. The patch is an early return at the top of the function.

reason: Since upstream #5017 (post `v0.9.1`) libghostty encodes every pane key,
and it sends unmodified `F13`-`F24` as rxvt-style `CSI 25~` .. `CSI 34~`
(`function_keys.zig`). The pane's `TERM=xterm-ghostty` terminfo declares
`kf13`-`kf24` as xterm's Shift+`F1`-`F12` (`\E[1;2P` .. `\E[24;2~`), so
terminfo-driven apps never recognise them as F13-F24. The patch rewrites
unmodified `F13`-`F24` to Shift+`F1`-`F12` and lets libghostty encode that.
Panes with Kitty keyboard or modifyOtherKeys 2 are untouched and still get
upstream's encoding.

history: before `v0.9.5` the patch was an `encode_f_key` arm in
`src/input/encode.rs`, because herdr's own encoder dropped F13 entirely.
Upstream deleted that file in #5017.

conflict risk: low. One early return in a small function plus one test.

remove when: libghostty's legacy F13-F24 sequences match the `xterm-ghostty`
terminfo `kf13`-`kf24` entries, or upstream remaps them itself. Check
`vendor/libghostty-vt/src/input/function_keys.zig` against
`vendor/libghostty-vt/src/terminfo/ghostty.zig`.

verification:

```sh
cargo nextest run --locked legacy_f13_to_f24_use_shifted_function_key_sequences
```

---

## 0004 pinned Main agent row (`[ui.main_agent]`)

status: active

upstream base: `ee5bfb50` (post `v0.9.5`)

local files (new, fork-only):

- `src/config/fork_main_agent.rs` — `MainAgentConfig`
- `src/client/shell/fork_main_agent.rs` — hiding, pinned row, toggle, click
- `src/client/shell/tests/fork_main_agent.rs`
- `src/server/headless/fork_main_agent.rs` — `client.main_agent.show`, `client.view.get`
- `src/server/headless/tests/fork_main_agent.rs`
- `src/cli/fork_main_agent.rs` — `herdr main-agent show|view`

touched upstream files (each hook is marked `// fork: FORK.md 0004`):

| file | anchor | hook |
| --- | --- | --- |
| `src/config.rs` | `mod` list and `pub use self::{` | declare and export `fork_main_agent` |
| `src/config/model.rs` | end of `struct UiConfig` and its `Default` | `main_agent` field |
| `src/client/shell.rs` | `mod` list | `mod fork_main_agent;` |
| `src/client/shell/state.rs` | end of `ClientShellConfig`; top of `ShellHitMap`; `hits:` in `ClientShellState` and its constructor | `main_agent`, `fork_main_agent` hit, `fork_main_agent_return` |
| `src/client/shell/config.rs` | `ClientShellConfig::from_config`; the `[ui]` block of `apply_live_config` | build `ClientMainAgent` (also publishes the hidden label) |
| `src/client/shell/sidebar.rs` | end of `workspace_entries`; top of `render_sidebar` after `render_sidebar_background` | `retain` filter; `render_pinned_row` |
| `src/client/shell/endpoint_sidebar.rs` | top of `render_expanded` after `render_sidebar_background` | `render_pinned_row` |
| `src/client/shell/mouse.rs` | left press, just before the workspace `group_toggle` lookup | `fork_main_agent_click` |
| `src/client/shell/input.rs` | `ClientShellMode::Terminal` before `resolve_direct_binding`; `ClientShellMode::Prefix` before `resolve_prefix_binding` | `fork_main_agent_key` |
| `src/client/shell/tests/mod.rs` | test `mod` list | `mod fork_main_agent;` |
| `src/api/schema.rs` | `define_operations!`, after `ClientWindowTitleClear` | `ClientMainAgentShow`, `ClientViewGet` |
| `src/api/schema/response.rs` | `ResponseResult`, after `ClientWindowTitle` | `ClientMainAgentShow`, `ClientView` |
| `src/app/api.rs` | app-mode `match`, before `SessionSnapshot` | both answer "no client" |
| `src/protocol/endpoint.rs` | after `AGENT_COMPLETIONS_KIND` | `FORK_MAIN_AGENT_SHOW_KIND` |
| `src/client/endpoint/control.rs` | `EndpointControlMessage`; top of `decode_endpoint_control` | `MainAgentShow` variant and decode |
| `src/client/mod.rs` | `EndpointControl` arm, before `EndpointControlMessage::Ignored` | `fork_main_agent_show_from_server` + `finish_client_shell_input` |
| `src/server/headless.rs` | `mod` list; `match` after `ClientWindowTitleClear` | `mod fork_main_agent;`, dispatch both methods |
| `src/server/headless/tests/mod.rs` | test `mod` list | `mod fork_main_agent;` |
| `src/cli.rs` | `mod` list; top-level `match command` | `"main-agent"` |
| `docs/next/api/herdr-api.schema.json` | generated | `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` |

reason: D36 in workflow-hub. The shared herdr app on the host holds Main (the
Hermes TUI) as a permanent, pinned sidebar entry above the machines and
spaces, and `SUPER+G` jumps to it.

behaviour:

```toml
[ui.main_agent]
workspace = "Main"   # label of a local workspace; empty (default) disables everything
title = ""           # row text, defaults to the label
key = "prefix+m"     # toggle; empty leaves it unbound
```

- The workspace with that label is filtered out of `workspace_entries`, which
  every expanded list, navigation, mouse and mobile path goes through.
- `render_pinned_row` takes two rows off the top of both expanded sidebars
  (local-only `render_sidebar` and the multi-machine `render_expanded`):
  `◆ <title> … running`, or `◇ … stopped` when the local server has no such
  workspace. Highlighted while it is the shown workspace on the local server.
- Click or `key` while elsewhere: remember `(endpoint, workspace)` and
  `focus_or_activate(Local, Workspace(main))`, so it also switches away from a
  remote machine. `key` on Main: return to the remembered place.
- `herdr main-agent show` (API `client.main_agent.show`): the local server
  forwards an endpoint control (`fork.main-agent.show.v1`) to its client
  shell, which shows Main like a click does: never toggles away, and works
  while the client shows another machine (endpoint controls pass even for
  inactive endpoints). Only the local endpoint's control is honoured.
  Returns `sent: false` when no client shell is connected.
- `herdr main-agent view` (API `client.view.get`): `attached`, `active`
  (the client presents this server, not another machine) and the
  `workspace_id` / `workspace_label` of the client's view on this server.
  Main is showing when `active` and the label is Main. Read from server
  state, no round trip to the client.
- Missing workspace: an `Unavailable` notice, nothing else. herdr never
  creates or restarts it; the `local.main-agent` plugin in nixos-work does
  that over the CLI.
- The key is matched before the regular bindings in Terminal and Prefix mode
  only (not in copy mode, navigate or resize). It is not part of `[keys]`, so
  `herdr config check`, the keybinding help and the settings UI do not list it.

design notes: the row, hiding and key are client-only. The CLI/API half
(added 2026-11-16) needs the server: SUPER+G used to send the key into the
window and read the outer window title to know whether Main showed, but in
the multi-machine client that title stayed the default `herdr`, so SUPER+G
on Main toggled away from it. The new API kinds are additive; an older
client ignores the unknown control. The hidden label is
a process-wide `RwLock<String>` set whenever the client config is built,
instead of a new `workspace_entries` parameter, so the eleven upstream call
sites stay untouched. Matching is by label on every endpoint: a remote
workspace labelled `Main` is hidden too.

known gaps: the collapsed sidebars (`render_collapsed_sidebar`,
`endpoint_sidebar::render_collapsed`) iterate `snapshot.workspaces` directly,
so Main shows there as a normal entry and has no pinned row. The navigator
(`goto`) and the workspace picker are not filtered either. No agent status:
Main runs behind `docker exec`, which herdr cannot detect, and the container
gets no herdr socket (D20/D36).

conflict risk: medium. Each hook is one or a few lines, but `render_sidebar`,
`render_expanded`, the mouse press handler and `route_key` are hot upstream
code. The config field is additive. On conflict, take upstream and re-add the
hooks from the table; the new files rarely need changes.

remove when: never upstream; this is a fork feature. Delete with the files and
hooks above if the pinned row is dropped.

verification:

```sh
cargo nextest run --locked main_agent
```

Manual (2026-11-16, debug build, isolated XDG dirs and socket, client in a
tmux pane, a throwaway `--remote-session` on devvm as second machine):
`show` from another workspace and from the VM lands on Main, `show` on Main
stays, `prefix+m` returns (also to the VM), `view` reports `active: false`
while the VM is shown.

---

## 0005 sidebar thread list (`[ui.threads]`)

status: active

upstream base: `ee5bfb50` (post `v0.9.5`)

local files (new, fork-only):

- `src/config/fork_threads.rs` — `ThreadsConfig`, `THREAD_PLACEHOLDER`
- `src/client/shell/fork_threads_feed.rs` — HTTP/SSE reader, parsing, sanitizing (+ stub service in its tests)
- `src/client/shell/fork_threads.rs` — thread block, command template, mapping, hiding, open plan, click, navigate keys
- `src/client/shell/tests/fork_threads.rs`
- `src/app/fork_threads.rs` — resume history token, `fork.resume` projection helper, `pane.run_command`
- `src/api/schema/fork_threads.rs` — `PaneRunCommandParams`
- `src/server/headless/tests/fork_threads.rs`

touched upstream files (each hook is marked `// fork: FORK.md 0005`):

| file | anchor | hook |
| --- | --- | --- |
| `src/config.rs` | `mod` list and `pub use self::{` | declare and export `fork_threads` |
| `src/config/model.rs` | end of `struct UiConfig` and its `Default` | `threads` field |
| `src/client/shell.rs` | `mod` list | `mod fork_threads; mod fork_threads_feed;` |
| `src/client/shell/state.rs` | end of `ClientShellConfig`; top of `ShellHitMap` | `threads`, `fork_threads` hits |
| `src/client/shell/config.rs` | `ClientShellConfig::from_config`; the `[ui]` block of `apply_live_config` | build `ClientThreads` (also starts/stops the reader) |
| `src/client/shell/sidebar.rs` | `render_sidebar` after the 0004 pinned row; end of `workspace_entries` after the 0004 `retain` | `render_sidebar_threads`; second `retain` |
| `src/client/shell/endpoint_sidebar.rs` | `render_expanded` after the 0004 pinned row | `render_sidebar_threads` |
| `src/client/shell/mouse.rs` | left press, after the 0004 click hook | `fork_threads_click` |
| `src/client/shell/input.rs` | top of `route_navigate_key`, before the Esc/prefix cancel; inside that cancel | `fork_threads_navigate_key`; clear the cursor |
| `src/client/mod.rs` | `ClientLoopEvent::Timer`, next to `tick_notifications` | `shell.fork_threads_tick(&mut outcome)` |
| `src/client/shell/tests/mod.rs` | test `mod` list | `mod fork_threads;` |
| `src/app/mod.rs` | `mod` list; after `pub use state::{…}` | `mod fork_threads;`, re-export the two token keys |
| `src/app/actions.rs` | `AppEvent::AgentResumeReported` arm of `handle_app_event` | `fork_record_session_command(pane_id)` after the update |
| `src/app/api.rs` | app-mode `match`, after `PaneClose` | dispatch `PaneRunCommand` |
| `src/api/schema.rs` | `mod`/`pub use` lists; `define_operations!` after `PaneClose` | `fork_threads`; `PaneRunCommand => "pane.run_command", true, true` |
| `src/api/schema/operations.rs` | `operation_ui_impact_preserves_existing_classification` | `"pane.run_command"` |
| `src/server/client_shell.rs` | agent projection, after `tokens` is collected | push `fork.resume` |
| `src/server/headless/client_views.rs` | the three `matches!` lists that contain `Method::PaneSplit(_)` | `Method::PaneRunCommand(_)` |
| `src/server/client_commands.rs` | `advertised_client_shell_method_shapes_stay_at_the_v1_contract`, after the `pane.link.resolve` freeze | freeze the `pane.run_command` digest |
| `src/server/headless/tests/mod.rs` | test `mod` list | `mod fork_threads;` |
| `docs/next/api/herdr-api.schema.json` | generated | `HERDR_UPDATE_API_SCHEMA=1 just test-one generated_protocol_schema_artifact_is_current` |

reason: D39 + D41 in workflow-hub. The host client lists the coding-agent
threads of tkd (on the dev VM) in the sidebar; each row opens its thread in a
workspace on the VM's herdr server, where `t3t -r <thread>` shows it.

behaviour:

```toml
[ui.threads]
url = "http://192.168.1.2:7420"  # plain http only; empty (default) disables everything
machine = "devvm"                # saved machine label that owns thread workspaces; "" = local server
command = "t3t -r {thread}"      # pane command; {thread} = thread id
```

- Data (D39, client only): one background thread per client process holds
  `GET <url>/api/events` (SSE) and refetches `GET <url>/api/sessions` on
  connect and whenever the `stamp` data changes. No polling; reconnect with
  1 s → 30 s backoff; a silent stream is dropped after 10 min. HTTP/1.0 over
  `std::net::TcpStream`, no new dependency. On failure the last rows stay,
  dimmed, header `offline`. Text is stripped of control and bidi/format
  characters and capped; rows whose id is not `[A-Za-z0-9_.:-]{1,128}` are
  dropped, because the id ends up in a shell command line.
- Rows: a block under the 0004 pinned row in both expanded sidebars, at most
  ~2/5 of the sidebar (`… N more` beyond, scrolls with the keyboard cursor).
  Ticket groups in order of their newest thread, `no ticket` last; herdr's
  status icons/colours (`running`→working, `blocked`/`error`→blocked, red and
  bold, with `[blocked]` after the title); agent·model on the right when the
  sidebar is 28+ cells wide. The thread on the focused pane of the thread
  machine is highlighted.
- Mapping (D41, server): every accepted `resume_argv` report appends the
  joined command to the workspace metadata token `fork.sessions` (one per
  line, newest last, max 64). The client-shell projection adds the pane's
  current resume command as agent token `fork.resume`. The client matches
  both against `command` (program basename, the word after the flag before
  `{thread}`; so `t3t --server x -r <id>` matches too). Keys contain a dot,
  which `*.report_metadata` rejects: API callers cannot forge or clear them.
  Workspace metadata is in-memory, so the mapping dies with the workspace.
- Hiding: `workspace_entries` drops a workspace whose history names a thread
  that is in the current list (any machine, by content). When none of its
  threads is listed any more it is a normal workspace again. Nothing closes.
- Open (click, or Enter in navigate mode): plan from the thread machine's
  snapshot. Owner workspace = the one whose history names the thread, else
  one that names a thread with the same `ticket.id`. Then: a pane whose
  resume is the thread → focus it (`pane.focus`); else a pane whose resume
  runs the command's program → restart in place; else add a pane; no owner →
  new workspace (`cwd` = thread cwd if it exists there, label = ticket label
  or title). The last three are one `pane.run_command` call. If the machine
  is not the shown one, the client shows it first (workspace/pane target)
  and finishes the open on the client timer once its surface is up (20 s
  limit).
- `pane.run_command` (new client-shell method, additive): composes
  `pane.split` next to `replace_pane_id` + `pane.close` of it (in-place
  restart), or `pane.split` of the workspace's focused pane, or
  `workspace.create`; then types the command + CR into the new shell (as
  agent resume restore does) and focuses the pane, which moves the client
  shell there. One line, no control characters, ≤4 KiB. Older servers do not
  advertise it; the client then shows "Update and restart it".
- Keyboard: in navigate mode (`prefix+w`) the rows sit above the first
  workspace in the up/down cycle: up from the first workspace enters at the
  last row, down from the last workspace at the first; started on a hidden
  thread workspace, up/down lands on that thread's row. Enter opens, Esc
  cancels.

design notes: reading is client-only (D39). The mapping is a server fact
(guardrail: runtime facts in server state), kept in existing workspace
metadata so no session or wire struct changes: snapshots are JSON and the
extra tokens ride in existing `tokens` fields. t3t reports with source
`t3code-tui`, which `agent_resume::is_official_agent_source` rejects, so
`agent_session` stays empty for it; the resume argv is the only place the
thread id lives, hence the projection token. `pane.run_command` exists
because the client-shell lane has no way to run a command in a pane
(`pane.send_text`, `agent.start` are not client-shell methods) and a
multi-step client flow across async endpoint responses would be far larger.
The in-place restart splits and closes instead of respawning the runtime,
whose old exit would race the new process. The rows, cursor and pending open
live in process-wide cells (like 0004's hidden label) so upstream render call
sites stay untouched.

known gaps: the history is not persisted: after a server restart (or live
handoff) a workspace only knows the threads its restored panes report again;
a no-ticket thread that had been switched away from then gets a new
workspace on its next open. Hiding matches any machine, so a local pane
reporting a listed thread hides that local workspace too, while rows always
open on `machine`. Collapsed and mobile sidebars, the navigator and the
workspace picker show no thread rows. While the cursor is in the rows,
a snapshot can re-select a workspace in navigate mode, so two rows may look
selected. The pane added by "add a pane" splits right of the focused pane;
there is no layout preference. A thread pane that never reported a thread
(t3t before its first thread) is not recognised as a thread pane.

conflict risk: medium, as 0004: one-line hooks, but `render_sidebar`,
`render_expanded`, `route_navigate_key`, the mouse handler, the client timer
arm and `handle_app_event` are hot upstream code; the operations table and
the frozen method-shape test change when upstream adds methods. On conflict,
take upstream, re-add the hooks from the table, re-freeze the digest if
upstream changed a referenced type, and regenerate the schema.

remove when: never upstream; fork feature. Delete with the files and hooks
above if the thread list is dropped. If upstream adds a pane-command
client-shell method or exposes resume commands in the projection, switch to
those and drop the server half.

verification:

```sh
cargo nextest run --locked threads
cargo nextest run --locked advertised_client_shell_method_shapes operation_ui_impact generated_protocol_schema
```

Manual (2026-11-16, debug build, isolated `XDG_CONFIG_HOME`/`XDG_STATE_HOME`
and `HERDR_SOCKET_PATH`, client in a private tmux server, `machine = ""`, a
Python stub of tkd's `/api/sessions` + `/api/events`, and a stand-in `t3t`
script on `PATH` that prints its thread and reports
`pane.report_agent {source: t3code-tui, agent: t3code, resume_argv: [t3t, -r, id]}`
like `t3code-tui/src/net/herdr.ts`): click on a thread without workspace →
new labelled workspace in its cwd, row highlighted, workspace hidden; second
thread of the same ticket → ticket workspace's pane restarted in place;
first thread again → restarted back (mapping outlived the pane); thread on
screen → focus only; after quitting the stand-in → pane added; `prefix+w`,
Up, Enter → opens; thread dropped from the list → its workspace reappears;
stub down → rows kept, `offline`; stub up → recovered; server restart →
history rebuilt from re-reports. Not verified: a real remote machine.

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
