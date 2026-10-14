# Clockout

CLI and TUI to track worked hours against a weekly goal, and see what time you can leave for the day.

## Usage

### TUI

```
clockout
```

In the footer you can see the what key does what.

### CLI

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

In the TUI, press `h` to toggle the history view, or `o` then `g` to manage
goal periods. To mark a day absent instead of logging time for it, open the
add popup (`a`) and press Alt+a to flip it into absence mode (date +
optional reason) - whole-day by default, or press Alt+t to fill in a time
interval instead (e.g. a couple of hours for an appointment).

## Build

Standard `cargo build --release`, or with Nix: `nix build` (or `nix run` /
`nix develop` for a dev shell).

## Config

Configuration in `~/.config/clockout/config.toml`. For example:

```toml
estimated_lunch_break_minutes = 30 # Assumed lunch break time, used for estimation
hide_weekends = false # Leave Saturday/Sunday out of the week diagram, day navigation, and history view
```

The weekly-hours goal isn't in `config.toml` - it lives in `entries.json` as
a list of dated periods, managed with `clockout goal ...` or the TUI's Goals
screen (`o` then `g`), so it can change over time (e.g. "6h/day starting
next Monday") without rewriting the time debt already accrued under the old
goal.

Absences (sick days, vacation, etc.) also live in `entries.json`, managed
with `clockout absence ...` or the TUI's add popup (Alt+a). A day marked
absent owes nothing toward the goal and doesn't accrue time debt; the TUI
also supports a partial-day absence (a time interval), which reduces that
day's target by the interval's duration instead of zeroing it.

## LLM disclosure

This program is heavily AI proompted.
