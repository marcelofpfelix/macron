# Personal Dashboards

Personal dashboards should be built on `boardd`, not inside Quickshell. Board
owns collection, caching, stale state, redaction, and actions. Quickshell owns
compact visual surfaces and explicit buttons.

## Surfaces

- `personal.today`: time, agenda, ready tasks, Pomodoro, current timewarrior
  interval, habits due today, sleep/run recovery summary.
- `personal.money`: hledger balances, month spending, ETF/BTC holdings summary,
  cached price age, and drift warnings.
- `personal.health`: sleep, runs, weekly load, recovery notes, and import age.
- `personal.habits`: habit streaks, due habits, missed habits, and quick check-in
  actions.

The bar should show only small status icons/counts. Details belong in a
Quickshell dashboard popup, `board tui`, and `board render text`.

## Source Policy

- `hledger` is the source of truth for balances and transactions.
- ETF/BTC prices are cached facts, not trading advice or automatic actions.
- Running/sleep data starts with local exports or official APIs only; no scraping
  credentials into QML.
- Habits start as local files or Taskwarrior/timewarrior tags before adding a
  separate app.
- Secrets stay in helpers or environment lookups; module output must be redacted
  before it reaches Quickshell.

## Module Plan

1. `native.hledger-summary`
   - Reads `hledger` output through bounded commands or a journal parser.
   - Emits balances, monthly spend, budget warnings, and last journal mtime.
   - Validation: fixture journal plus `board check hledger-summary`.

2. `native.market-prices`
   - Caches ETF/BTC prices with explicit source, timestamp, and stale state.
   - Emits holdings values only after combining with local hledger symbols.
   - Validation: offline fixture prices and stale-cache rendering.

3. `native.pomodoro`
   - Stores local timer state: stopped, focus, short break, long break, paused.
   - Optional links: Taskwarrior task UUID and active timewarrior tag.
   - Actions: start, pause, resume, stop, complete.
   - Validation: state-machine tests and `board action pomodoro.start` dry run.

4. `native.timewarrior-current`
   - Reads the active `timew` interval and totals for today.
   - Emits current tag/project and elapsed time.
   - Validation: fake `timew export` fixture.

5. `native.health-import`
   - Imports sleep/running summaries from local export files first.
   - Later supports official-source helpers for COROS, Strava, Health Connect,
     or Garmin when credentials and APIs are confirmed.
   - Validation: fixture export with no raw private health details in output.

6. `native.habits`
   - Reads a small local habit state file or Taskwarrior tags.
   - Emits due, done, missed, streak, and today check-in actions.
   - Validation: fixture state and action dry run.

## Actions

Actions must be explicit and reversible where possible:

- `pomodoro.start`, `pomodoro.pause`, `pomodoro.resume`, `pomodoro.stop`.
- `habit.check-in <id>`.
- `hledger.open-journal`.
- `market.refresh-prices`.
- `health.import`.

No financial trades, no automatic health uploads, and no hidden credential
refresh from a render path.

## Quickshell Shape

Quickshell should render one dashboard with tabs:

- Today
- Money
- Health
- Habits

Each tab reads board state. Buttons call `board action ...`. Quickshell should
not run `hledger`, fetch prices, parse health exports, or keep raw private data
in QML state.

## Initial Tasks

1. Add `personal.today` and `personal.money` surfaces to board config examples.
   - Acceptance: surfaces reference only modules that have cacheable structured
     output.
   - Validation: `board doctor` and `board render text personal.today`.

2. Design the Pomodoro state machine before implementation.
   - Acceptance: documented states, transitions, action names, persistence file,
     and optional Taskwarrior/timewarrior links.
   - Validation: state-machine tests can be written from the note without UI.

3. Add hledger fixture-based summary module.
   - Acceptance: module emits redacted balances, monthly spend, and stale state.
   - Validation: fixture journal test plus `board check hledger-summary`.

4. Add market price cache module.
   - Acceptance: prices are cached, stale-aware, source-attributed, and never
     fetched from render.
   - Validation: offline fixture prices and cache-age tests.

5. Add local health import prototype.
   - Acceptance: one local export format imports sleep/run summary and hides raw
     details from Quickshell output.
   - Validation: fixture import test.

6. Add habit state prototype.
   - Acceptance: due/done/missed/streak and check-in action work from a local
     file.
   - Validation: fixture state plus action dry run.

7. Add Quickshell personal dashboard shell.
   - Acceptance: tabs render board state and show unavailable/stale states.
   - Validation: `qs-menu-smoke personal-dashboard` and `desktop-doctor`.
