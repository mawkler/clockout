# Clockout

CLI and TUI to track worked hours against a weekly goal, and see what time you can leave for the day.

## Usage

### TUI

Run it without any arguments to start the TUI:

```
clockout
```

In the footer you can see the what key does what.

### CLI

There's also a CLI:

```
Usage: clockout [COMMAND]

Commands:
  log      Log a worked time range, e.g. `clockout log 8:00-12:00`
  start    Start an open-ended block, e.g. `clockout start 12:35` (default: now)
  end      Close the open block started with `clockout start` (default: now)
  done     Close the open block (if any) and show where you stand for the day
  status   Print a one-shot plain-text summary
  tui      Launch the interactive dashboard (also the default with no subcommand)
  list     List logged entries
  history  Show weekly totals, one week per row and one day per column
  edit     Edit an existing entry by id
  delete   Delete an entry by id
  goal     Manage the weekly-hours goal over time
  absence  Record absences (sick days, vacation, etc.) that don't accrue time debt
  help     Print this message or the help of the given subcommand(s)

Options:
  -h, --help     Print help
  -V, --version  Print version
```

## Running/installing

Run with Nix:

```sh
nix run github:mawkler/clockout
```

Alternatively, install with cargo:

```sh
git clone https://github.com/mawkler/clockout
cargo install --path clockout/
```

## Config

Configuration lives in `~/.config/clockout/config.toml`. Here's the default config:

```toml
# Assumed lunch break time, used for estimating "leave at" before you've had your break(s)
estimated_lunch_break_minutes = 30
# Hide Saturday and Sunday
hide_weekends = false
# Round suggested time to the nearest 1 minutes
round_to_minutes = 1
```

## LLM disclosure

This program is mostly AI proompted.
