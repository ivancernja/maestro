# maestro

Run parallel coding agents in isolated git worktrees, from one terminal app.

[![ci](https://github.com/ivancernja/maestro/actions/workflows/ci.yml/badge.svg)](https://github.com/ivancernja/maestro/actions/workflows/ci.yml)
[![license](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)
[![rust](https://img.shields.io/badge/rust-1.88%2B-orange.svg)](https://www.rust-lang.org)

![maestro](docs/hero.png)

A workspace is a git worktree on its own branch, with a coding agent running
inside it. maestro lists the workspaces in a sidebar and shows the selected
agent's terminal next to it. The terminal is a real one, so you can type to the
agent directly.

Each agent has a separate checkout, so they do not touch each other's files.
The agents run in detached tmux sessions, which means they keep running when you
quit maestro and are still there when you open it again.

## Install

```bash
cargo install --git https://github.com/ivancernja/maestro
```

Or build it from a clone:

```bash
git clone https://github.com/ivancernja/maestro
cd maestro
cargo build --release
ln -s "$PWD/target/release/maestro" ~/.local/bin/maestro
```

| Needs | For |
| --- | --- |
| `tmux`, `git` | required, a workspace is a tmux session in a worktree |
| an agent CLI | any of claude, codex, opencode, gemini, copilot, crush, grok, omp, pi |
| `ss` (iproute2) | the listening ports panel |
| `lazygit` | reviewing changes with `g` |

Repositories are found under `~/Work`, `~/code`, `~/src`, `~/dev`, `~/projects`
and similar directories without configuration.

## Keys

Run `maestro`. Normal mode drives the app, and the main pane always shows
whatever is selected, live.

| Key | Action |
| --- | --- |
| `j` `k` | move, and switch the main pane to that agent |
| click, scroll | select a workspace with the mouse |
| `⏎` `i` | insert mode, every key goes to the agent |
| `n` | new workspace |
| `g` | lazygit over the worktree |
| `s` `e` | `$SHELL` or `$EDITOR` in the worktree |
| `p` | listening ports |
| `L` | merge into the base branch and retire the workspace |
| `R` | relaunch an agent that has exited |
| `d` | remove the workspace, worktree and branch |
| `r` | refresh now |
| `esc` | dismiss an error |
| `q` | quit, agents keep running |

Three keys stay yours while typing to an agent, so switching and creating never
needs a trip back through normal mode.

| Key | Action |
| --- | --- |
| `alt+n` | new workspace |
| `alt+j` `alt+k` | switch agent and keep typing |
| `ctrl+q` | leave insert mode, or close an open tool |

## Creating a workspace

`n` asks for the repository, the branch, the agent, and the task.

![new workspace](docs/new.png)

The branch field is pre-filled with a composer's name that is not already used
in that repository. `⏎` accepts it, `tab` picks a different one, and typing
replaces it. The worktree is created next to the repository as
`../<repo>--<branch>`.

The task is optional. If you give one, maestro passes it to the agent on startup
with whatever prompt flag that agent takes, so it begins working right away. If
you leave it empty, the agent opens at its prompt.

## Status

Status comes from whether an agent's screen changed since the last poll. A moving
spinner is work, a still screen is not.

| Mark | Meaning |
| --- | --- |
| `●` | working |
| `◆` | went quiet while you were looking elsewhere |
| `○` | quiet, and you have seen it |
| `✗` | the agent exited, `R` restarts it |

`◆` is the only state that needs you, so it is the only one that sends a desktop
notification. Selecting the workspace clears the mark. If the agent was already
selected when it went quiet, it is not marked, because you were looking at it.

Neither state distinguishes "waiting for your approval" from "finished", since
both leave a still screen. That would need per-agent transcript reading.

## Review and land

`g` opens lazygit over the worktree, which covers the diff, staging, committing
and pushing. `L` then merges the branch into the branch it was created from and
retires the workspace.

Because the merge happens in the repository you work in, maestro checks the
preconditions first and says which one failed. The worktree has to be clean, the
branch has to be ahead of its base, and the main checkout has to be on that base
and clean. If any check fails it does nothing.

## Listening ports

`p` shows what is listening, read from `ss` and `/proc`.

![listening ports](docs/ports.png)

Dev servers lead the list and are marked, matched on the command line rather than
the process name, so `next-server` reads as `next` and `encore run` as `encore`
instead of all of them saying `node`. Each row carries the git root it was started
in and how long it has been up, and the selected row's pid and command line sit
underneath.

`x` stops one. It asks first and names the port, then sends SIGTERM and escalates
to SIGKILL if that is ignored. Only your own sockets appear, because `ss`
withholds the pid for everyone else's.

## Configuration

| Variable | Default |
| --- | --- |
| `MAESTRO_REPO_ROOT` | discovered, colon separated for several roots |
| `MAESTRO_STATE` | `~/.local/state/maestro` |
| `MAESTRO_SESSION_PREFIX` | `mst-` |
| `MAESTRO_THEME_FILE` | the active theme's `colors.toml` |

## How it works

- tmux hosts the agents and is never shown. Each workspace is a detached session,
  and maestro attaches to the selected one inside a pty sized to the main pane,
  rendered by [tui-term](https://github.com/a-kenji/tui-term). That is why agents
  survive quitting, and why keystrokes reach them unmodified.
- Panes are addressed by absolute tmux id, because a `base-index` of 1 makes
  numeric targets like `:0.0` fail to resolve.
- Worktrees follow the `../<repo>--<branch>` convention, so they are
  interchangeable with hand-made ones.
- On [Omarchy](https://omarchy.org) the palette is resolved with
  `omarchy-theme-color`, the same resolver the config templates and terminal use,
  and `omarchy theme set` is picked up within a second. Everywhere else the colors
  are ANSI names, which inherit whatever the terminal defines.

## Uninstall

```bash
cargo uninstall maestro          # or remove the symlink you made
rm -rf ~/.local/state/maestro    # the workspace registry
```

Workspaces themselves outlive the binary. Remove them with `d` before
uninstalling, or clean up by hand:

```bash
tmux ls | grep '^mst-' | cut -d: -f1 | xargs -r -n1 tmux kill-session -t
git -C <repo> worktree list      # then `worktree remove` and `branch -d`
```

## Development

```bash
cargo build
cargo test        # prompt composition and shell quoting
./test/run        # 72 tests driving the real binary inside tmux
```

`test/run` launches maestro in tmux and asserts on what the panes actually
render, including pty sizing, workspace switching, key forwarding, mouse clicks
and the ports panel. `MAESTRO_AGENT_CMD` substitutes a stand-in process so the
suite never spends tokens on a real agent.

## License

MIT, see [LICENSE](LICENSE).
