# Clockout

CLI and TUI to track worked hours against a weekly goal, and see what time you can leave for the day.

## Usage

```text
clockout                       launch the TUI dashboard
clockout start [HH:MM]         start an open block (default: now)
clockout end [HH:MM]           close the open block (default: now)
clockout log 8:00-12:00        log a finished range (--day/--date for other days)
clockout status                one-shot text summary
clockout list [--all]          list entries (this week, or --all)
clockout history               weekly totals, one week per row, one day per column
clockout edit <id> [...]       edit an entry
clockout delete <id>           delete an entry
clockout goal set <hours>      set the weekly-hours goal, effective from a date (--from, default: today)
clockout goal list             list goal periods
clockout goal edit <id> [...]  edit a goal period
clockout goal delete <id>      delete a goal period
```

In the TUI, press `h` to toggle the history view, or `g` to manage goal periods.

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
a list of dated periods, managed with `clockout goal ...` or the TUI's `g`
screen, so it can change over time (e.g. "6h/day starting next Monday")
without rewriting the time debt already accrued under the old goal.

## LLM disclosure

This program is heavily AI proompted.
