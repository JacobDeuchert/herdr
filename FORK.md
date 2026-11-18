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
- `src/client/shell/fork_threads_feed.rs` — HTTP/SSE reader, parsing of the session list and its `inbox` object, fallback, timestamps, sanitizing (+ stub service in its tests)
- `src/client/shell/fork_threads_inbox.rs` — t3code inbox rules: sections, sort, shelves/caps, row labels, recede, next clock change (pure)
- `src/client/shell/fork_threads.rs` — panel rendering (expanded and collapsed), command template, mapping, hiding, open plan, click, agent key bindings, navigate cursor, timer
- the ticket picker files, see the addendum below
- `src/client/shell/tests/fork_threads.rs`
- `src/app/fork_threads.rs` — resume history token, `fork.resume` projection helper, `pane.run_command`
- `src/api/schema/fork_threads.rs` — `PaneRunCommandParams`
- `src/server/headless/tests/fork_threads.rs`

touched upstream files (each hook is marked `// fork: FORK.md 0005`):

| file | anchor | hook |
| --- | --- | --- |
| `src/config.rs` | `mod` list and `pub use self::{` | declare and export `fork_threads` |
| `src/config/model.rs` | end of `struct UiConfig` and its `Default` | `threads` field |
| `src/client/shell.rs` | `mod` list | `mod fork_threads; mod fork_threads_feed; mod fork_threads_inbox;` |
| `src/client/shell/state.rs` | end of `ClientShellConfig`; top of `ShellHitMap` | `threads`, `fork_threads` hits |
| `src/client/shell/config.rs` | `ClientShellConfig::from_config`; the `[ui]` block of `apply_live_config` | build `ClientThreads` (also starts/stops the reader) |
| `src/client/shell/sidebar.rs` | `render_sidebar`, just before `super::render_agent_panel(`; `render_collapsed_sidebar`, just before the `for … ordered_agent_pane_ids` loop; end of `workspace_entries` after the 0004 `retain` | `let detail_area = render_thread_panel(…)`; `let detail_content = render_collapsed_threads(…)`; second `retain` |
| `src/client/shell/endpoint_sidebar.rs` | `render_expanded`, just before `endpoint_agents::render_expanded(`; `render_collapsed`, just before `endpoint_agents::render_collapsed(` | `let detail_area = render_thread_panel(…)`; `let detail_area = render_collapsed_endpoint_threads(…)` |
| `src/client/shell/actions.rs` | `record_binding`, just before `handle_endpoint_navigation` | `fork_threads_agent_action` |
| `src/client/shell/input.rs` | top of `indexed_navigation_target_exists`; top of `route_navigate_key`, before the Esc/prefix cancel; inside that cancel | `fork_threads_target_exists`; `fork_threads_navigate_key`; clear the cursor |
| `src/client/shell/mouse.rs` | left press, after the 0004 click hook | `fork_threads_click` |
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

How the agents section is replaced: the four render hooks shadow the agents
section's area (`detail_area` / `detail_content`) with a zero-height rect
when they drew the thread panel, so upstream's agent rendering that follows
draws nothing; with `[ui.threads]` unset they return the area unchanged.
The panel writes the agents section's own hit fields (`agent_body`,
`agent_scrollbar`, `agent_scroll_metrics`, `agent_max_scroll`) and uses its
scroll offset (`ShellRenderState::agent_scroll`), so upstream's wheel,
scrollbar-drag and section-divider handlers in `mouse.rs` work on it
unchanged. It never sets `agent_sort_toggle`, `hits.agents` or
`hits.endpoint_agents`. Key actions are taken before upstream sees them:
`fork_threads_agent_action` returns true for `previous_agent`, `next_agent`
and `focus_agent(n)` whenever the feature is on (also with no rows), so
neither `handle_endpoint_navigation` nor `endpoint_method_for_action` runs.

reason: D39 + D41 in workflow-hub. The host client lists the coding-agent
threads of tkd (on the dev VM) in the sidebar, in place of herdr's agent
list and in the order of t3code's own sidebar; each row opens its thread in
a workspace on the VM's herdr server, where `t3t -r <thread>` shows it.

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
- Contract: each session keeps tkd's top-level fields and carries an `inbox`
  object (`listed, pinned, settled, raised_hand, working, snoozed_until,
  raised_hand_woke_at, pin_order_key, inbox_sort_at, working_sort_at,
  settled_sort_at, activity_at, status, goal_active, unread,
  last_visited_at, working_started_at, waiting_since, project_title, pr,
  receded`; tkd does not ship `woke_at`, the client derives it).
  Parsed leniently: unknown fields ignored, missing or mistyped ones take
  defaults (`listed` true), times are ISO 8601 or Unix ms. Top-level
  `status` is the coarse tkd status; `inbox.status` the row status. An item
  without `inbox` (tkd before the inbox change) is listed, in no shelf but
  Working (top-level `running`), status from the top-level field
  (`blocked` → Approval, or Input when `pending_kind` names input; `error`
  → Failed; `done` → Done), every sort key = `updated_at`.
- Panel (rules of t3code's Sidebar V2, spec in workflow-hub's session
  scratchpad `t3-sidebar-rules.md` §1–5, §8): in the agents section's place
  (below the machines/spaces list, same rule + ` threads` header, same
  share of the sidebar and section divider). Rows with `listed` false are
  dropped. Section = snoozed (`snoozed_until > now && !raised_hand`) →
  settled → pinned → working → active. Top to bottom: the inbox, then
  the Working section. The inbox is pinned rows and active rows (no
  headers), capped together at 8 with `Show N more` / `Show less` (fork
  rule, not t3code's; rows past the cap stay when they are on screen or
  need attention: raised hand, Approval, Input, Request), then `Snoozed N`
  (only when non-empty) and `Settled N` (always, unless the list is
  empty); shelves start closed, sit at the bottom of the inbox area when
  everything fits, and toggle by click or Enter in navigate mode. A closed
  shelf still shows the thread on screen. Open Settled shows 10 rows (plus
  the one on screen) and `Show N more` (+25). Working is not a shelf
  (fork rule, workflow-hub D41 amendment 2026-11-17): its own block at the
  bottom of the panel, a rule and ` working N`, always open, up to half
  the panel, not scrolled with the inbox; drawn only when non-empty. Sort: pinned by `pin_order_key` ascending, keyless
  by `created_at` desc; active by `inbox_sort_at`, working by
  `working_sort_at`, settled by `settled_sort_at`, all desc; snoozed by
  `snoozed_until` asc (stable); ties by id. Shelf state, the inbox
  expansion and page size live in the client process only.
- Rows: pinned/active/working are cards: icon, title, right-aligned label,
  and a dim second line (ticket label · project · branch · `#N state`) when
  there is one. Label: Approval / Input / Request / Waiting + duration /
  Setting up + duration / Working or Goal + duration / Limited / Failed,
  then for `ready`: Woke (`raised_hand_woke_at`, else a passed
  `snoozed_until`, newer than `last_visited_at`, not settled), Done
  (`unread`), else relative time of `activity_at`. Snoozed/settled rows are
  slim (one line, dot icon, no status): `◷ Nm|Nh|Nd` countdown, Woke, or a
  relative time (`settled_sort_at` for settled). herdr's icons/colours:
  approval/input/request/limited/failed use `blocked` (red) with a bold
  title and a coloured label (approval/limited peach, input mauve, request
  teal, failed red); preparing/working/waiting use `working`; unread or
  woke `done`; else `idle`. Receding rows get a dimmer title: tkd's
  `receded` flag, minus the clock-dependent Woke term for ready/approval
  rows; without the flag, working states and ready/approval rows that are
  neither unread nor woke. The thread on the
  focused pane of the thread machine is highlighted.
- Clock: snooze ends, Woke, relative times, durations and countdowns are
  computed from the raw timestamps against the wall clock at render time.
  The client timer (100 ms) only compares `now` with the stored next change
  (`next_clock_change`) and repaints when it is reached; the deadline is
  recomputed then and on every new list.
- Collapsed sidebars: the displayed rows as `N` + status icon in place of
  the agents' list; click opens.
- Input, all taken from the agents section when the feature is on:
  `next_agent` / `previous_agent` step through the displayed rows in order
  (wrapping; starting from the thread on screen, else first/last) and open
  the row like a click, scrolling it into view; `focus_agent` N opens the
  Nth displayed row (no-op beyond the last, and
  `indexed_navigation_target_exists` reports it missing). Click opens a
  row, toggles a shelf, or shows more. Mouse wheel and scrollbar scroll the
  panel. Navigate mode (`prefix+w`): the panel's lines follow the last
  workspace in the up/down cycle (down from the last workspace enters the
  first line, up from the first workspace enters the last line, leaving the
  ends returns to the workspaces); started on a hidden thread workspace,
  up/down lands on that thread's row; Enter opens a row (and leaves
  navigate mode) or toggles a shelf / shows more (stays); Esc cancels.
- Mapping (D41, server): every accepted `resume_argv` report appends the
  joined command to the workspace metadata token `fork.sessions` (one per
  line, newest last, max 64). The client-shell projection adds the pane's
  current resume command as agent token `fork.resume`. The client matches
  both against `command` (program basename, the word after the flag before
  `{thread}`; so `t3t --server x -r <id>` matches too). Keys contain a dot,
  which `*.report_metadata` rejects: API callers cannot forge or clear them.
  Workspace metadata is in-memory, so the mapping dies with the workspace.
- Drafts (2026-11-17, see the addendum): `workspace_entries` also drops a
  workspace with the token `fork.draft`, and the panel lists it as a draft
  row above the inbox.
- Hiding: `workspace_entries` drops a workspace whose history names a thread
  that is listed (any machine, by content). When none of its threads is
  listed any more it is a normal workspace again. Nothing closes.
- Open (click, bindings, Enter): plan from the thread machine's snapshot.
  Owner workspace = the one whose history names the thread, else one that
  names a thread with the same `ticket.id`. Then: a pane whose resume is the
  thread → focus it (`pane.focus`); else a pane whose resume runs the
  command's program → restart in place; else add a pane; no owner → new
  workspace (`cwd` = thread cwd if it exists there, label = ticket label or
  title). The last three are one `pane.run_command` call. If the machine is
  not the shown one, the client shows it first (workspace/pane target) and
  finishes the open on the client timer once its surface is up (20 s
  limit).
- `pane.run_command` (new client-shell method, additive): composes
  `pane.split` next to `replace_pane_id` + `pane.close` of it (in-place
  restart), or `pane.split` of the workspace's focused pane, or
  `workspace.create`; then types the command + CR into the new shell (as
  agent resume restore does) and focuses the pane, which moves the client
  shell there. One line, no control characters, ≤4 KiB. Older servers do not
  advertise it; the client then shows "Update and restart it".

design notes: reading is client-only (D39). The inbox rules run in the
client on fields tkd precomputes (spec §8): everything that changes only on
a thread event is shipped as a flag or sort key, everything that flips with
the clock as a raw timestamp. The mapping is a server fact (guardrail:
runtime facts in server state), kept in existing workspace metadata so no
session or wire struct changes: snapshots are JSON and the extra tokens ride
in existing `tokens` fields. t3t reports with source `t3code-tui`, which
`agent_resume::is_official_agent_source` rejects, so `agent_session` stays
empty for it; the resume argv is the only place the thread id lives, hence
the projection token. `pane.run_command` exists because the client-shell
lane has no way to run a command in a pane. The in-place restart splits and
closes instead of respawning the runtime, whose old exit would race the new
process. The rows, cursor, shelf state and pending open live in
process-wide cells (like 0004's hidden label) so upstream render call sites
stay untouched; the panel reuses the agents section's hit fields and scroll
offset instead of adding its own.

known gaps: the history is not persisted: after a server restart (or live
handoff) a workspace only knows the threads its restored panes report again.
Hiding matches any machine, so a local pane reporting a listed thread hides
that local workspace too, while rows always open on `machine`. The mobile
layout (narrow terminal) still lists herdr's agents in its switcher, though
the agent bindings drive threads there too. The navigator and the workspace
picker show no threads. The agents section's sort toggle
(`agent_panel_sort`) has no counterpart. Not reproduced from t3code: the
2 s working exit grace and observed return order are tkd's job (spec §7);
project scope, drafts, search, hover actions, drag and drop. While the
cursor is in the panel a snapshot can re-select a workspace, so two lines
may look selected. The pane added by "add a pane" splits right of the
focused pane. A thread pane that never reported a thread is not recognised.

conflict risk: medium: one-line hooks, but `render_sidebar`,
`render_collapsed_sidebar`, `render_expanded`, `record_binding`,
`route_navigate_key`, the mouse handler, the client timer arm and
`handle_app_event` are hot upstream code; the operations table and the
frozen method-shape test change when upstream adds methods. On conflict,
take upstream, re-add the hooks from the table (the render hooks go
immediately before the agents call so the shadowed area feeds it),
re-freeze the digest if upstream changed a referenced type, and regenerate
the schema. If upstream renames the agents section's hit fields or scroll
offset, follow the rename in `render_panel`; if it adds a new input path
for the agents section (another key action, a context menu), mirror it in
`fork_threads.rs`.

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
and `HERDR_SOCKET_PATH`, client in a private tmux server, `machine = ""`,
keys `next_agent = "prefix+j"`, `previous_agent = "prefix+k"`,
`focus_agent = "alt+1..9"`, a Python stub of tkd's `/api/sessions` +
`/api/events` serving `inbox` objects for every section and status (and a
`flat` mode without them), and the stand-in `t3t` that reports
`pane.report_agent {source: t3code-tui, resume_argv: [t3t, -r, id]}`):
panel below spaces, no agents section; pinned (keyed first), active rows
with Approval/Input/Request/Failed/Limited/Done/Woke/time; Working 4,
Snoozed 2, Settled 14 shelves closed; Enter and click toggle them; Settled
shows 10 + `Show 4 more`, which shows the rest; a snooze ending 90 s after
generation moved the row to Active as Woke with no new fetch; countdown
`◷ 1m`; `prefix+j` from nothing opened the first row (new hidden
workspace), again the second, `prefix+k` focused the first's pane, `alt+3`
opened the third row in a workspace labelled by its ticket; click on a row
opens; wheel scrolls; `flat` data → one active list by `updated_at` with
`Working 1` and `Settled 0`; stub down → `offline`, rows kept; stub up →
recovered. Not verified: a real remote machine, the real tkd.

### 0005 addendum: ticket picker (`[ui.threads] new_key`), projects, drafts

Part of 0005; reapply it after the main entry. Added 2026-11-17 (decided by
Jacob the same day).

local files (new, fork-only):

- `src/client/shell/fork_tickets.rs` — ticket parsing, filter, order and
  groups, open plan, picker state and keys, composition hook
- `src/client/shell/fork_tickets_overlay.rs` — the overlay (child of
  `overlays.rs`, so it reuses `panel`, `popup`, `contrast`)
- `src/client/shell/tests/fork_tickets.rs`

touched files (each hook is marked `// fork: FORK.md 0005`):

| file | anchor | hook |
| --- | --- | --- |
| `src/client/shell.rs` | `mod` list | `mod fork_tickets;` |
| `src/client/shell/overlays.rs` | `mod worktree_overlays;` | `mod fork_tickets_overlay;` + re-export `fork_render_ticket_picker` |
| `src/client/shell/render.rs` | `pub(super) use overlays::{…}` | re-export `fork_render_ticket_picker` |
| `src/client/shell/input.rs` | top of `route_key_press`; Terminal and Prefix arms, after the 0004 hooks | `fork_tickets_picker_key`; `fork_tickets_key` (Direct / Prefix) |
| `src/client/shell/overlay_input.rs` | top of `insert_overlay_text` | `fork_tickets::insert_picker_text` |
| `src/client/shell/mouse.rs` | top of `handle_mouse` | `fork_tickets_mouse` |
| `src/client/shell/composition.rs` | after the `if let Some(overlay) = self.overlay` block of `compose` | `self.fork_tickets_compose(&mut frame, &mut occlusion)` |
| `src/client/shell/tests/mod.rs` | test `mod` list | `mod fork_tickets;` |

plus, inside the 0005 files: `ThreadsConfig::{new_key, new_command,
project_command, new_keybinds}`, `TICKET_PLACEHOLDER` and `DIR_PLACEHOLDER`
(exported from `src/config.rs`); `DRAFT_TOKEN` (re-exported from
`src/app/mod.rs` as `FORK_DRAFT_TOKEN`); `InboxLine::Draft`;
`ClientThreads::{new_key, new_command, project_command}`; the reader fetches tickets
(`ensure_reader(url, tickets)`, `FeedState::tickets*`); `fork_threads_open`
split into `fork_threads_machine` + `fork_threads_run` (shared with the
picker); `replace_or_add` split out of `plan_open`; the pending open holds an
`OpenTarget` (`Thread(id)` | `NewForTicket(label)` | `NewForProject(id)`).

behaviour:

```toml
[ui.threads]
new_key = "prefix+t"                                  # empty (default): picker off
new_command = 'd="$(tk dir {ticket})" && cd "$d" && exec t3t'   # default; {ticket} = ticket label
project_command = "cd {dir} && exec t3t"              # default; {dir} = project root, single-quoted
```

- Data: with `new_key` set, the reader also fetches `GET <url>/api/tickets`
  on every new stamp (after `/api/sessions`). A failing ticket fetch (an
  older tkd) only marks the picker offline. Done and dropped tickets are
  dropped; so are items whose `label` is not `t<digits>` (it ends up in a
  shell command line). Title, status and `extRef` are sanitized like thread
  titles; `dir` is kept only when absolute and free of control characters.
- Projects (added 2026-11-17): the reader also fetches `GET
  <url>/api/projects` (tkd: t3 projects that are not ticket dirs, most
  recently active first) after the tickets; a failing fetch keeps the last
  list and does not mark the picker offline. Items without an id, with a
  root that is not absolute, has a control character or a `'`, and repeats
  are dropped; an empty title becomes the root's last component.
- Order: started tickets (in-progress / in-review, then any other
  non-parked status), then a dim, unselectable `parked` header and the
  parked ones (todo, then draft); label number descending inside each; then
  a dim `projects` header and the projects in the service's order. A header
  is left out when its group is empty after filtering. The filter also
  matches project titles and roots; the count reads `N tickets · M
  projects`.
- Picker: `new_key` in Terminal or Prefix mode (checked before the regular
  bindings, like 0004's key, so it wins over `[keys]` and over a shown
  machine's `[[keys.command]]`) opens a modal centred over the current view,
  styled like the worktree picker: filter line, rows `label status title …
  extRef`, the selected ticket's action (`new workspace t13`, `new thread
  in place of the thread pane in X`, `new thread pane in X`), key hints.
  Typing filters (every word must be a substring of label, title or ref,
  case-insensitive); up/down and ctrl+n/ctrl+p move (the cursor never lands
  on the header); Enter starts the thread; Esc or ctrl+c closes. It takes
  every key, committed text and paste while open; a mouse press closes it.
  Loading and offline (with the error when there is no list yet) show in
  the picker.
- Enter: plan on the thread machine's snapshot. The ticket's workspace is
  the one whose history names a thread whose row has that `ticket.label`,
  else one labelled with the ticket (a workspace the picker created before
  its t3t reported a thread). It has a thread pane → restart it in place
  (focused one first, `OpenPlan::Replace`), else add one (`AddTo`); no
  workspace → new workspace labelled with the ticket, `cwd` = the ticket's
  `dir` when set. Command: `new_command` with `{ticket}` replaced after the
  label passed `^t[0-9]+$`. Same `fork_threads_run` path as a thread row,
  including showing the machine first and finishing on the client timer.
- Enter on a project: always a new workspace labelled with the project
  title, `cwd` = the root, command `project_command` with `{dir}` replaced
  by the root in single quotes. Same run path (`OpenTarget::NewForProject`).
- Drafts: a workspace `pane.run_command` creates (picker or a thread row
  without a workspace) gets the server token `fork.draft` = the requested
  label; the server removes it when a pane in the workspace reports a
  resume command. The client hides such a workspace from the workspace
  lists at once (any machine, by content, while `[ui.threads]` is on) and
  draws the thread machine's drafts as one-line rows above the inbox:
  `◌ label … draft`, highlighted when it is the focused workspace of the
  shown thread machine. Click or navigate-mode Enter shows the workspace
  (`workspace.focus` via `focus_or_activate`); navigate mode started on a
  draft workspace lands on its row. Drafts are not in the
  `next_agent`/`focus_agent` cycle and not in collapsed sidebars. Reason:
  without it the new workspace sat in the workspace list until the first
  prompt and then jumped to the thread panel (workflow-hub D41 amendment
  2026-11-17).
- `pane.run_command` types the command + CR into the new pane's
  interactive shell (`pane.send_text`), so `new_command` is shell syntax
  and needs no `sh -c`. A fresh `t3t` is a draft and reports its thread
  only after the first prompt; until then its pane is not a thread pane.

design notes: the picker is not a `ClientShellOverlay` variant (that enum is
matched all over upstream input, mouse and render code); it is a
process-wide cell like the thread view, drawn by one composition hook after
upstream's overlays, and its input is taken first in `route_key_press`.
Repeats of a key the picker consumed are reprocessed by upstream's input
leases through the same path. Key precedence: the client-side key is
matched before `resolve_prefix_binding`, which also holds the shown
endpoint's `[[keys.command]]` bindings (merged into the client keymap for
the `Local` keybinding source), so `prefix+t` opens the picker even while
the VM's own `prefix+t` (`local.tk-board.open`) is bound there. It is not
part of `[keys]`: no collision diagnostic, not in help or settings. A
`new_key = "prefix+shift+t"` would silently shadow upstream's default
`rename_tab`.

known gaps: a draft whose t3t quits without a thread stays a draft row
until its workspace is closed. A project thread never joins an existing
workspace. The project list is refetched on stamps only, so a new t3
project shows once any session changes. Clicks do not select rows (a press only closes). No scrollbar,
no page up/down. No new thread when the ticket list is empty or offline
with nothing cached. A pane whose draft t3t has not reported a thread is not
replaced; Enter adds a second pane next to it.

remove when: with 0005.

verification:

```sh
cargo nextest run --locked tickets threads
```

Manual (2026-11-17, debug build, isolated XDG dirs, `HERDR_SOCKET_PATH` in
`/tmp` (the scratchpad path was too long for a unix socket; debug builds
read `herdr-dev/config.toml`), client in a private tmux server, `machine =
""`, a Python stub of tkd (`/api/events`, `/api/sessions`, `/api/tickets`)
and stand-ins for `t3t` (reports `t3t -r <id>` when resumed, else prints
"draft") and `tk dir` (mkdir + print)): picker with the stub down shows
`offline` and the connection error; after starting the stub it filled in on
its own with t11/t6, `parked`, t13/t14, closed t2 absent, bidi stripped;
Enter on t6, whose thread runs in a workspace labelled `six`, replaced that
pane in place (old pane closed, new draft t3t in `tk dir t6`); `conf` →
only t13 under `parked`, Enter created workspace `t13` with the draft t3t in
its new dir; picker on t13 again offers `new thread pane in t13` (label
fallback); stub down → `offline`, rows kept. Not verified: a remote thread
machine (covered by a unit test only), the real tkd and t3t.

Projects and drafts (2026-11-17): unit tests only (`cargo nextest run
--locked tickets threads`, 60 pass; the server test checks the token is set
on create and cleared by the first resume report). Not run against the
real tkd, t3t or devvm yet.

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
