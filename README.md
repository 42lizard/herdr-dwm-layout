# DWM Layout for Herdr

`42lizard.dwm-layout` is a DWM-style master/stack layout plugin for
[Herdr](https://herdr.dev/). It keeps one or more panes in a master area and
places the remaining panes in an equally sized stack.

The plugin is written in Rust and rearranges existing Herdr panes without
restarting their processes. Live PTYs, running commands, pane IDs, terminal
IDs, and scrollback remain attached to their panes during a reflow.

## Features

- Horizontal layout: masters on the left, stack on the right.
- Vertical layout: masters on top, stack below.
- Configurable number of master panes (`nmaster`).
- Configurable master-area ratio (`mfact`).
- Swap the focused pane with the master area.
- Automatic reconciliation after panes are created or closed.
- Independent state for every managed Herdr tab.
- Atomic, lock-protected state updates.
- No runtime dependencies beyond the compiled binary and Herdr.

## Requirements

- Herdr 0.8.2 or newer.
- Linux or macOS.
- Rust 1.85 or newer and Cargo when building from source.

Rust is only required to build the plugin. The compiled release binary has no
Rust runtime dependency.

## Installation

### From GitHub

Once this repository is public, install it with Herdr:

```sh
herdr plugin install 42lizard/herdr-dwm-layout
```

Herdr displays the manifest and build command for review, runs the declared
release build, and registers the plugin. Confirm the installation:

```sh
herdr plugin list --plugin 42lizard.dwm-layout --json
herdr plugin action list --plugin 42lizard.dwm-layout
```

If the plugin lives in a subdirectory of a larger repository, append that
subdirectory to the install source:

```sh
herdr plugin install 42lizard/repository-name/path/to/dwm-layout
```

### Local development checkout

`herdr plugin link` does not run manifest build commands. Build first, then
link the plugin directory:

```sh
git clone https://github.com/42lizard/herdr-dwm-layout.git
cd herdr-dwm-layout
cargo build --release --locked
herdr plugin link "$PWD" --enabled
```

Re-run `herdr plugin link "$PWD" --enabled` after changing
`herdr-plugin.toml`, because Herdr validates and registers the manifest at link
time.

## Keybindings

Plugins do not modify `~/.config/herdr/config.toml` automatically. Add the
bindings you want under your existing Herdr configuration. The following map
matches the original tmux/DWM workflow and assumes `Ctrl+Space` as the prefix:

```toml
[keys]
prefix = "ctrl+space"

[[keys.command]]
key = "prefix+|"
type = "plugin_action"
command = "42lizard.dwm-layout.split-right"
description = "DWM split right"

[[keys.command]]
key = "prefix+minus"
type = "plugin_action"
command = "42lizard.dwm-layout.split-down"
description = "DWM split down"

[[keys.command]]
key = "prefix+enter"
type = "plugin_action"
command = "42lizard.dwm-layout.swap-master"
description = "DWM swap focused pane with master"

[[keys.command]]
key = "prefix+e"
type = "plugin_action"
command = "42lizard.dwm-layout.next-layout"
description = "DWM toggle master orientation"

[[keys.command]]
key = "prefix+i"
type = "plugin_action"
command = "42lizard.dwm-layout.increase-nmaster"
description = "DWM increase master count"

[[keys.command]]
key = "prefix+o"
type = "plugin_action"
command = "42lizard.dwm-layout.decrease-nmaster"
description = "DWM decrease master count"

[[keys.command]]
key = "prefix+>"
type = "plugin_action"
command = "42lizard.dwm-layout.increase-mfact"
description = "DWM grow master area"

[[keys.command]]
key = "prefix+<"
type = "plugin_action"
command = "42lizard.dwm-layout.decrease-mfact"
description = "DWM shrink master area"
```

Validate and apply the configuration:

```sh
herdr config check
herdr server reload-config
```

Key syntax and availability can depend on the outer terminal and desktop
environment. Run `prefix+?` inside Herdr to inspect the active bindings.

## Usage

The example configuration provides these shortcuts:

| Shortcut | Action |
| --- | --- |
| `Ctrl+Space`, then `|` | Create a pane, promote it to master, and normalize the layout |
| `Ctrl+Space`, then `-` | Create a pane, promote it to master, and normalize the layout |
| `Ctrl+Space`, then `Enter` | Swap the focused pane with the opposite layout group |
| `Ctrl+Space`, then `e` | Toggle horizontal/vertical orientation |
| `Ctrl+Space`, then `i` | Increase the number of master panes |
| `Ctrl+Space`, then `o` | Decrease the number of master panes |
| `Ctrl+Space`, then `>` | Increase the master area by 5 percentage points |
| `Ctrl+Space`, then `<` | Decrease the master area by 5 percentage points |

Both split actions create a new pane using the focused pane's working
directory and promote the new pane into the master area, matching the original
tmux behavior. The final position is determined by the active DWM orientation, so
the resulting managed layout is normalized regardless of the initial split
direction.

The first DWM split automatically enables layout management for the current
tab. Existing tabs can also be enabled explicitly:

```sh
herdr plugin action invoke 42lizard.dwm-layout.enable
```

Disable automatic management without changing the current pane layout:

```sh
herdr plugin action invoke 42lizard.dwm-layout.disable
```

All actions can be invoked without keybindings:

```sh
herdr plugin action list --plugin 42lizard.dwm-layout
herdr plugin action invoke 42lizard.dwm-layout.next-layout
herdr plugin action invoke 42lizard.dwm-layout.increase-nmaster
herdr plugin action invoke 42lizard.dwm-layout.decrease-mfact
```

## Layout model

Each managed tab stores the following values:

| Value | Default | Behavior |
| --- | ---: | --- |
| `orientation` | `horizontal` | `horizontal` places masters left; `vertical` places masters on top |
| `nmaster` | `1` | Number of panes assigned to the master area; clamped to the current pane count |
| `mfact` | `0.50` | Fraction of the tab assigned to the master area |

`mfact` changes in steps of `0.05` and is clamped to `0.10..=0.90`. Panes
inside the master and stack groups receive equal shares of their group. When
all panes are masters, there is no stack boundary; the stored `mfact` takes
effect when a stack pane exists again.

### Horizontal

```text
┌─────────────────────┬─────────────────┐
│ master 1            │ stack 1         │
├─────────────────────┼─────────────────┤
│ master 2            │ stack 2         │
└─────────────────────┴─────────────────┘
```

### Vertical

```text
┌─────────────────────┬─────────────────┐
│ master 1            │ master 2        │
├─────────────────────┴─────────────────┤
│ stack 1                               │
└───────────────────────────────────────┘
```

## How live reflow works

Herdr's `layout.apply` creates replacement terminals, so this plugin does not
use it for managed panes. A structural reflow instead:

1. Leaves the first master pane in the original tab.
2. Moves the other live panes to a temporary tab named `dwm-staging`.
3. Moves those panes back into the requested master/stack BSP tree.
4. Lets Herdr remove the empty staging tab automatically.

`pane.move` preserves each live terminal. The operation may cause a brief UI
redraw, but it does not intentionally restart commands or agents.

Plugin actions are serialized with a file lock so simultaneous key actions and
event hooks cannot update the same state file concurrently.

## State and lifecycle

Herdr provides a private state directory through `HERDR_PLUGIN_STATE_DIR`. The
plugin writes:

```text
$HERDR_PLUGIN_STATE_DIR/
├── state.json
└── state.lock
```

`state.json` contains one entry per managed tab. It records orientation,
`mfact`, `nmaster`, last-used master/stack panes, and the last reconciled pane
IDs. Writes use a temporary file followed by an atomic rename.

The plugin listens for:

- `pane.created`: reconcile externally created panes in managed tabs.
- `pane.closed`: clamp state and rebuild the remaining layout when necessary.
- `tab.closed`: remove state belonging to the closed tab.

Disabling a tab removes its entry from plugin state. Unmanaged tabs are never
reflowed by event hooks.

## Updating

For a GitHub installation, reinstall the same source:

```sh
herdr plugin install 42lizard/herdr-dwm-layout
```

For a linked checkout:

```sh
git pull --ff-only
cargo build --release --locked
herdr plugin link "$PWD" --enabled
```

State is stored outside the source checkout and survives a rebuild or relink.

## Troubleshooting

### A binding does nothing

Check configuration and registration:

```sh
herdr config check
herdr plugin list --plugin 42lizard.dwm-layout --json
herdr plugin action list --plugin 42lizard.dwm-layout
```

Then reload the running server:

```sh
herdr server reload-config
```

If the binding still does not fire, use `prefix+?` to check for a conflicting
binding and verify that the outer terminal or desktop has not consumed the key.

### The release binary is missing

This usually means a locally linked checkout was not built:

```sh
cargo build --release --locked
test -x target/release/herdr-dwm-layout
```

### An action fails

Inspect Herdr's plugin command log:

```sh
herdr plugin log list --plugin 42lizard.dwm-layout
```

Errors from the binary are prefixed with `dwm-layout:` and written to stderr.
Also verify that the active Herdr server is compatible:

```sh
herdr status server
```

### A `dwm-staging` tab remains visible

This indicates that a reflow was interrupted between pane moves, for example
because Herdr or the plugin process was terminated. The panes are live; do not
close the staging tab until its panes have been moved somewhere safe. Use
Herdr's pane menu or `herdr pane move` to return them to the intended tab, then
close the empty staging tab and invoke `enable` again.

### Reset layout management for one tab

Disable and enable the focused tab:

```sh
herdr plugin action invoke 42lizard.dwm-layout.disable
herdr plugin action invoke 42lizard.dwm-layout.enable
```

This resets the tab to default plugin state and immediately normalizes it as a
horizontal layout with one master and `mfact = 0.50`.

## Uninstalling

For a GitHub-managed installation:

```sh
herdr plugin uninstall 42lizard.dwm-layout
```

For a local development link:

```sh
herdr plugin unlink 42lizard.dwm-layout
```

Remove or update any `42lizard.dwm-layout.*` keybindings in
`~/.config/herdr/config.toml`, then run `herdr server reload-config`.

Herdr keeps plugin-owned config and state unless explicitly removed. Preserve
`state.json` if you may reinstall and want to retain tab settings.

## Development

Build and run the checks from the repository root:

```sh
cargo fmt --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
cargo build --release --locked
```

Run the same checks without network access after dependencies are cached:

```sh
cargo test --locked --offline
cargo clippy --locked --offline --all-targets -- -D warnings
cargo build --release --locked --offline
```

The test module lives next to the implementation in `src/main.rs`. It covers
BSP pane ordering, single-pane state, horizontal master/stack construction,
and migration of state written by the former Python implementation.

### Source layout

```text
.
├── Cargo.lock
├── Cargo.toml
├── README.md
├── herdr-plugin.toml
└── src/
    └── main.rs
```

### Manual integration test

Before publishing a release, verify both orientations in an isolated Herdr
workspace containing at least three disposable shell panes:

1. Record the pane IDs and terminal IDs with `herdr pane list`.
2. Enable the plugin and exercise `next-layout`, `increase-nmaster`, and both
   `mfact` actions.
3. Confirm the expected BSP tree with `herdr pane layout`.
4. Confirm all original pane IDs and terminal IDs still exist.
5. Close the disposable workspace.

Do not use running production agents as integration-test panes.

## Publishing checklist

Before the first public release:

- Confirm the MIT copyright holder and year in `LICENSE`.
- Keep the versions in `Cargo.toml` and `herdr-plugin.toml` synchronized.
- Run formatting, tests, Clippy, and the release build on Linux and macOS.
- Run the manual live-reflow test above against the minimum supported Herdr
  version and the current stable release.
- Test `herdr plugin install 42lizard/herdr-dwm-layout` from a clean checkout.
- Add the `herdr-plugin` GitHub repository topic so the Herdr marketplace can
  discover the plugin.
- Tag the release and document user-visible changes.

## Compatibility and support

The manifest currently declares Linux and macOS support and requires Herdr
0.8.2 or newer. Windows is not supported because the implementation uses Unix
domain sockets and Unix file locking.

When reporting a problem, include:

- OS and architecture.
- `herdr --version`.
- The relevant entry from `herdr plugin log list --plugin 42lizard.dwm-layout`.
- Pane count, orientation, `nmaster`, and `mfact`.
- Whether a `dwm-staging` tab remained after the failure.

## License

Licensed under the [MIT License](LICENSE).

Copyright (c) 2026 42lizard.
