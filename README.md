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
```

In the TUI, press `h` to toggle the history view.

## Build

Standard `cargo build --release`, or with Nix: `nix build` (or `nix run` /
`nix develop` for a dev shell).

## Config

Configuration in `~/.config/clockout/config.toml`. For example:

```toml
weekly_goal_hours = 40.0  # Target hours to work every week
estimated_lunch_break_minutes = 30 # Assumed lunch break time, used for estimation
hide_weekends = false # Leave Saturday/Sunday out of the week diagram, day navigation, and history view
```

## LLM disclosure

This program is heavily AI proompted.
