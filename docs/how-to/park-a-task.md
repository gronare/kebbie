# Park a task until later

Set aside a task you are not ready to act on, so the board and the lead keep your focus on what matters now.

1. Select the task on the board and press `Z`. The `Park <task> until` window opens with `Tomorrow` selected, each choice showing the time it means, such as `sat 09:00`.

2. Pick when it comes back:

   - `⏎` keeps `Tomorrow`: it comes back at 09:00 tomorrow.
   - `2`, `Monday`: it comes back on Monday at 09:00.
   - `3`, `A date…`: type a day as you would for a later start, such as `fri`, `24 nov` or `2026-12-01`, and the window shows the time it reads, such as `thu 5 nov 09:00`, then `⏎`. A day alone means 09:00 that day; `tomorrow 13:00` sets the time too.
   - `4`, `I bring it back`: it stays parked until you unpark it.

   From a terminal, the same in one line:

   ```
   kebbie park billing --until fri
   ```

3. The task leaves its group for one line under the Work column's groups, such as `◌ 1 parked · Z shows them`. It keeps working, the lead leaves it out of its summaries, and kebbie sends no notification for it.

4. Wait: when its time comes, it returns to its group by itself. To bring it back sooner, press `Z` on the parked line to show the parked tasks, then `Z` on the task, or run:

   ```
   kebbie unpark billing
   ```

You see it worked when the task's row is gone from its group and the parked line counts it.

- If a parked task asks a question or its CI fails, it stays parked: the line says so, as `◌ 1 parked · 1 question · Z shows them`, and `i` still opens the question.
- If you drop, reset or finish a parked task, its park ends with it.

See also: [Parked](../reference/board.md#parked) and [`kebbie park`](../reference/commands.md#kebbie-park-name---until-when).
