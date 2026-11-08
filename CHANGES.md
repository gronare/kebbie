# Changes

What each kebbie release brings, one line per change, as you would tell someone who uses kebbie. The board's update window shows the entries of the update it installed; scripts/release.sh moves the Unreleased entries under the new version.

## Unreleased

### What's new
- Every release carries a CycloneDX SBOM, kebbie-<version>.cdx.json, next to the binaries.

### Fixed
- A test command written inside a heredoc, such as a plan written with cat <<EOF, is no longer recorded as a test run.
- A test run whose result line counts no tests, such as a filter that matched nothing, shows as unknown, never as passed.
- A long test command that ends in the background is judged on its whole line, so a cut-off | tail no longer reads as a pass.
- A test run that passes in the background sends its own task back to the reviewer, never the task whose worktree the shell is in.
- A task's shell commands stay quick while a background test run is pending, and a run whose end never shows stays unknown after a day.
- Test runs that end at the same moment in parallel shells are all recorded.
- A test run recorded in a task without the sandbox reads unverified: recorded without a sandbox, wherever it shows.

## 0.36.0

### What's new
- kebbie issue report --all, or a in the board's bugs to send, sends every waiting bug on one yes, paced to the endpoint's 5 per 5 minutes and waiting out a busy answer.

### Fixed
- kebbie records a test run after cd x; or a separate VAR=$(…); statement, and through timeout, nice or time.
- A task hears at once when kebbie leaves a test-like command unrecorded, and why.
- A test command piped into tail or grep is judged by its result line, and recorded as unknown without one, never as passed.
- A test run that goes to the background shows as unknown until it ends, then with its real result.
- Headless Chrome and Playwright start in a task, whose temp folder now has a short path in /tmp.
- A review that only asks for your confirmation waits under Needs you with its finding, and your answer there passes it; a task held by a review's findings shows the first one on its row.
- A worktree git cannot read shows its error on its own row under Needs you, and the rest of the board shows as usual.
- A task that rewords a commit its review asked changes for goes back to the reviewer when it reports ready, instead of keeping the push refused.
- A ready task held only by uncommitted files names them on its row, and its push window opens ready to push the commits alone; approve says the same.
- A pushed task's menu no longer offers rebase, tidy, push, let the lead push or keep rebased, and G re-runs its failed or cancelled CI.
- A follow-up started for a comment runs in the permission mode of the task started last, unless a profile or config sets one.
- A prompt on a task's screen reaches the lead when its turn ends, without waiting out a conversation with you.
- A task whose work belongs in another repo asks the lead for a half there instead of making a worktree by hand that Push cannot see.
- A push's rebase that meets another git's index.lock half-way waits and goes on, instead of failing on its own rebase.
- kebbie runs one rebase at a time in a task's worktree, and leaves a rebase it did not start running instead of aborting it.
- A ready task can name a task to push before it with kebbie report --after, and Next's push group puts that one first and shows "after <task>"; a wait counts across repos.
- Update restarts every board it finds running, in any Space, and kebbie doctor names a board still on an older kebbie.

## 0.35.0

### What's new

- Error signals and the install signal are off until you say yes in kebbie setup.

### Fixed

## 0.34.2

### What's new

### Fixed
- The board's Areas page and vault backlog read mindin's index in ~/.cache/mindin on a new machine.

## 0.34.1

### What's new

### Fixed
- kebbie update --check finds the vault plugin's latest version when the plugin lives in another repo than its marketplace.

## 0.34.0

### What's new
- kebbie setup is six steps: this machine, agents, vault, backlog, error signals and one review of every write, grouped by tool; each agent found shows its version, sign-in and plugin, and every ticked agent gets kebbie's plugin and approval rules without a question
- The vault step of kebbie setup asks where the vault lives, your own server, an S3 bucket, a local folder or none, preselected from what the vault answers, and runs mindin setup for a new place as you apply, installing mindin first when no command on the machine can; the command kebbie runs is its own pick, shown once, and the vault reaches kebbie's [vault] command, the Claude vault plugin and Codex's config.toml, each listed on the review
- kebbie setup --yes applies the guide's defaults, herdr's keys and each ticked agent's plugin and rules among them, printing every write; kebbie setup codex --yes also writes Codex's rules file and vault server
- kebbie reaches the vault through its own [vault] command, the same for tasks in Claude Code, Codex or Cursor: kebbie setup finds the vault commands in the Claude vault plugin, Codex's and Cursor's MCP config and on PATH
- kebbie asks the vault command what it is with about --json and makes only the calls its stated capabilities cover; a vault without about, such as notes-vault-mcp up to 0.13, is read from its status and help as before
- kebbie doctor in a terminal shows what needs you first, each with its fix, then one line per group (kebbie, Claude Code, Codex, Vault, herdr, Tools, Files, Tasks, Signals) with what its checks found; kebbie doctor --all shows every check under its group, and --plain, NO_COLOR or output that is not a terminal keeps the ok, warn and fail lines
- kebbie doctor shows the vault command's package, version and capabilities, and says when it fails, lacks a capability or its package has a new name; kebbie doctor --fix writes the suggested [vault] command after your y
- kebbie update --check names the vault plugin's package as the plugin pins it and its command answers, and the vault plugin is known by its vault server rather than by a package name, so updates keep finding both after a rename
- A release action's bump marker can name {package}, the repo's package name from pyproject.toml, Cargo.toml or package.json, so a renamed package keeps its release action
- A Claude task is asked by the vault plugin to save what you or the lead told it once a few messages have gone without a vault write; it still skips the log check, since kebbie writes a task's log lines (needs the vault plugin release that brings this)

### Fixed
- A restarted Codex task runs with its task's profile, sandbox and permissions as kebbie starts it today, so a task an older kebbie started can write its worktree's git folder again without asking for every commit

## 0.33.0

### What's new
- The board reads the vault from a snapshot the vault server sends where the server has one, so the Areas page, the backlog and the ongoing notes work with a vault hosted on a server and no local index; the keeper fetches it every 20 seconds, or the board while the keeper is off
- A plan kept in a vault note shows on the board without a local vault index too, read through the vault server when the plan is opened

### Fixed
- Codex 0.164 and later keep trusting kebbie's hooks across kebbie updates, so a Codex task starts without Hooks need review; Codex asks to trust them once more after this update
- The Areas page shows its areas in about 0.2 s, where it took close to a second, and in 40 s or more on a machine whose vault had not named its repos' log notes yet
- After herdr restores its Spaces, as after a reboot, kebbie restarts the lead and each task that herdr resumed without kebbie's role and settings, in the same conversation, so the lead keeps its commands and tasks their hooks; until then the keeper counts such a lead as stale, and a refused command and kebbie peers tell the lead that L on the board, Resume this conversation, restores it
- A rules break holds only while the task's commits are the ones the rules check read, so splitting or rewording a commit gets a fresh check even when every file stays the same, where the old break was told again on a commit that no longer existed

## 0.32.0

### What's new
- The hub protocol signs, seals and pairs under kebbie's own domain strings, and the board sends its version as kebbie; a machine needs this release to reach hub.kebbie.dev, which speaks only this protocol
- kebbie doctor checks the task hooks: it names open sessions and the lead whose hooks run a kebbie binary that no longer exists, and tells you to run kebbie restart

### Fixed
- A Codex task with task.sandbox = false can write the package caches, ~/.npm, ~/.cache and ~/.cargo or a profile's allow_write, so npm ci in its worktree no longer stops to ask you

## 0.31.0

### What's new
- kebbie start --no-note starts a task without a vault note, for a throwaway task
- task.require_accept = true holds every push until each commit it takes is accepted: kebbie push, the board's push and push in order refuse with the first step to accept and how, and a ready task with one waits under Needs you as accept step N; off by default, and no effect with task.steps off
- v on a task opens a review menu, review · <task>, with its commits since main and the assessor's word: v sends it to the reviewer, and on a task in review v withdraws it; with review.second set, s reviews with a second opinion beside the reviewer and o runs the second opinion alone
- review.profile picks who reviews, the built-in reviewer or a review profile of yours, and its verdict holds the push; review.model is that reviewer's model; the Settings page picks it from the review profiles whose agent you set up
- review.second asks a second review profile, a Codex one for instance, for its opinion: offered with s, or "always <profile>" with every review; it never holds the push, its points show on the task's row as second opinion: 2 points · v i reads them, i in the v menu pages through them, and the lead passes them to the task as a review's findings; kebbie review --second and --second-only run it from the shell
- The new-profile steps ask What is it for? after the name: a review profile starts from a review profile, takes only an agent, model, effort and role, written as kind = "review", and is never a task's start or task.profile
- A review profile can run Codex: codex exec in Codex's read-only sandbox, without your Codex config, MCP servers, plugins or rules, started by the keeper with the codex on its own PATH
- Only the agents you chose in setup are offered where a profile's agent is picked: without Codex set up, the new-profile steps offer Claude Code alone and no Codex profile as a start or as task.profile, and a Codex profile copied from another machine shows ⚠ Codex is not set up: kebbie setup codex on the Settings page, in kebbie profiles and kebbie doctor, and kebbie start --profile refuses it with the same words
- The board's Settings page has a Profiles section: kebbie's own profiles, task, reviewer and ops, under FROM KEBBIE apart from yours under YOURS, each with its agent, model, effort and role, the agent's own default model shown dim and a ⚠ on a Codex profile whose model Codex does not offer your account, and the selected one shows its role in two layers: kebbie's role from the installed kebbie, which updates with it, then your role file read after it; m picks the model from Codex's own list for your account or Claude Code's opus, sonnet, haiku and fable, e the effort from the levels that model takes, r on one of yours its role from the role files in ~/.config/kebbie/profiles with the profiles that share each, a new empty one or a copy, and on one of kebbie's shows kebbie's role read-only or adds your lines in profiles/<name>.md, which no update writes; d goes back to the agent's defaults and ⏎ opens the file, and a pick writes only that key, a built-in profile getting a file of its own with it; the reviewer's row shows review.model when it is set, and m there writes review.model; n adds a profile in six steps and a check, as a routine is added: its name, the profile it starts from, Claude Code or Codex, its model and effort from the same lists as m and e, and its role, a new empty file, a copy of the start's or a role file other profiles share, taking the start's about, for, skills, permissions and sandbox, never kebbie's role, and y writes it once it loads; x deletes one of yours after a y / n, its role file too when no other file in the folder names it, and keeps kebbie's own, one an open task runs on, one task.profile names in any config and one a routine starts its tasks with
- Every pick on the Settings page, a config key's values, a profile's model and effort and task.profile, asks in the form of the new-routine steps: a question, what it means, the answers with the one in effect marked now, as an answer of its own when the list does not offer it, and the keys under them, titled as model · codex-build, and a click chooses or closes it as on the Routines page
- task.profile on the Settings page is a pick of the profiles there are, each with what it is for, and none (task) removes the key, where any name could be typed; kebbie doctor warns of a task.profile in config.toml or a repo config that names no profile, with the profiles there are
- A Codex profile takes Codex's own effort levels, ultra, none, minimal and persistent among them, where kebbie refused any but Claude Code's five; a Claude profile still takes low, medium, high, xhigh or max
- kebbie restarts a Codex task that runs older code, as it does a Claude one: after kebbie update, after a newer codex lands on PATH, or when it runs without kebbie's plugin, it resumes the task's conversation with codex resume once it is idle, out of focus, its composer empty and no start-up question showing, where kebbie restart left every Codex as it ran; kebbie restart lists them under Claude and Codex sessions
- kebbie profiles shows the model a Codex profile without model runs, from Codex's own config.toml, as model gpt-5.5 from Codex's config.toml, and kebbie doctor names it on a codex model line
- The lead plans each task by its playbook, the skill kebbie:playbook: nine recipes, from a small fix with its test to a feature with a mockup you approve before its plan and build, each with when it applies and where you are needed; the lead answers a question about why something was built from the task's note and the vault, archive included, not from the board's tasks
- The lead names how each task is done when it starts one, as kebbie start --recipe "small-fix: one function and its test" from nine recipes, and the board shows recipe: small fix under the task's row with its why on the open task; kebbie recipe lists the recipes, shows a task's or changes it, and the choice goes to the task's note, in the vault or in kebbie's own notes folder when the task has no vault note; the follow-up a comment's card starts carries the recipe follow-up
- On a ready report, a short read-only assessor judges whether the task's whole work, its pushed steps included, needs an independent review, by its size, its complexity, its tests, the database, business logic and security, and the task's row says why, as review suggested: changes how VAT is rounded and its tests · v or no review: wording in two templates; review.auto = "suggest" (the default) leaves the review to you and the lead, "run" starts the reviewer without holding the push, and "require" holds the push until it passes
- kebbie doctor and the setup guide's Agents step say whether Claude Code and Codex are logged in, from claude auth status and codex login status, with claude auth login or codex login as the fix; an agent that gives no answer reads as login not checked
- The setup guide gives each agent you choose its own steps after Agents: Claude Code's plugin, login, approvals, vault and updates, and Codex's plugin, approvals, vault server and updates; kebbie setup claude and kebbie setup codex open it on that agent's steps alone, Review and apply included, and nothing is written before ⏎ there
- Each setup step that writes another tool's file offers kebbie does it, I'll add it myself or Skip; what you add yourself is shown on the last screen as exactly what goes where, and printed in full when you quit, and kebbie doctor names it until it is in place, a task running either way
- Codex's Approvals step writes ~/.codex/rules/kebbie.rules, which lets the ten commands a task runs, kebbie report, tell, ask and steps among them, past Codex's approval question; kebbie push, approve and review still ask
- With kebbie's vault on, Codex gets the vault plugin's own server in its config.toml, appended with the rest of your file as it is, so a Codex task keeps its own note; the S3 keys never go in the file: Codex reads them by name from its environment, kebbie starts Codex tasks in a tab whose shell has them, never on a command line, and the task's own commands don't get them; kebbie doctor fails its codex vault line while the server is missing
- kebbie checks once a day for a newer Codex, read where Codex's own update check reads it, and in Codex's model list for a model a profile names that is gone or has a newer one in its place and for a change of Codex's default; each finding is one notice on the board and a line in kebbie doctor
- After U updates kebbie, an agent updates window offers a newer Codex beside how it is installed, and ⏎ runs Codex's own installer; update.agents = "auto" installs it with U and says so, "notify" shows only the notice, and Codex tasks restart on the new version once they are idle
- A task whose profile names a Codex model Codex no longer lists starts on the model Codex names in its place, else on Codex's default, with a notice on the board and a word to the lead; the profile file stays as you wrote it
- kebbie doctor warns of a Claude profile that names a full model id, which keeps its tasks on that model, where an alias (opus, sonnet, haiku, fable) follows Claude Code to each new one
- The lead tells you kebbie's own task commands are safe to let through when an agent asks before one, and recommends its don't-ask-again answer
- N on the board opens Next: what needs you in one numbered list, in the order to do it, under STOPPED ON YOU (the task most others wait behind first, then the one waiting longest), PUSH, IN THIS ORDER (what other tasks of its repo wait on first, two tasks in the same files in the order they were built, then the one ready first) and TO READ, with how long each has waited and a line counting what is working, queued and parked; ⏎ opens the question, the push or the card, and P pushes the push group in that order after a window that shows it, the first that is refused stopping the rest; with nothing waiting it says Nothing needs you and what is on its way in
- kebbie next prints what needs you, numbered in the order to do it, and the lead answers a question about status or what to push first with it
- The phone shows what the board shows: Next above the groups in the order N gives it, per machine; each task's title, agent and model, recipe and review assessment; a prompt on an agent's screen, Codex's approvals among them, under Questions with its command and options, counted in the badge; the task's review section with whether the push waits for the reviewer; the queued and parked tasks, routines and away mode; and a task's steps with how many wait for your accept and which is next only under task.require_accept, otherwise that push takes what is committed. Update the hub first: an older hub's web app refuses the new board

### Fixed
- A push whose rebase git stops for a reason other than a conflict, such as no user.email in git, says git's own reason and leaves the branch as it was, where it was handed to the task as a conflict with nothing to resolve
- kebbie restart no longer takes a session for one running an older Claude Code or Codex when the agent was installed in the same second the session started
- A Codex start whose trust question is drawn after herdr calls Codex ready says Codex asks whether you trust its folder and holds the prompt, where it said Codex's input box already held "1. Trust and continue"
- A Codex task commits, stashes and rebases without asking, and reaches kebbie's sandbox hosts, npm and crates.io among them, where every commit and every npm ci waited on an approval: with task.sandbox on it runs under kebbie's own Codex permission profile, which writes what a commit writes in .git but never its config, hooks or push block, and reaches only the sandbox's hosts through Codex's network proxy; with task.sandbox off, Codex's workspace-write gets the git dirs and the network
- A Codex task's row says its Codex runs the tests, resolves the conflict, fixes the red CI, is taking a turn or says the failure is outside the code, where it said its Claude, and a task whose agent is gone says no agent
- The update window scrolls a list of what's new and fixed taller than the screen with ↑ ↓, PgUp PgDn, Home End and the mouse wheel, its keys line staying at the bottom with ↓ more while there is more, where the list was cut and the wheel closed the window
- A Codex task's title on the board is the title Codex gives its conversation alone, where it read as Action Required | Fix the flaky checkout test | wt1.app, with Codex's status and the worktree's name around it; a title kebbie already kept that way shows the conversation's title
- A Codex task that stops on an approval or a choice mid-task, its terminal titled [ ! ] Action Required, waits under Needs you and the lead hears the question with its options and the keys Codex shows, where the task showed under Working · nothing yet; kebbie choose takes an option's number or its key, as kebbie choose <task> y --prompt <id>
- A kebbie command a sandboxed Codex task ran inside Codex's sandbox says to run it again asking to leave the sandbox, which you or the lead approve, where it told Codex to drop cd and pipes as a Claude task must
- A Codex task with a vault note sends the lead the note's Problem, its decisions and its solution with kebbie tell lead, and the lead writes them, kebbie's recipe and assessor lines among them, where its role sent it looking for vault tools a Codex session does not have and it stopped to ask; it puts its plan to you as a file
- An API error that stops a Codex task's turn puts the task under Needs you with the API's own message, as stopped by a model that is not available: The 'gpt-5.1-codex-max' model is not supported when using Codex with a ChatGPT account., and the lead hears it, where the task showed idle · nothing yet
- kebbie types into a Codex composer only when it shows nothing or Codex's own placeholder, and a Codex start whose composer already holds text keeps the task's prompt until it is empty, where the prompt was glued onto the text already there
- A Codex task on one of Codex's start-up questions waits under Needs you with what Codex asks, and the lead hears it, after a restart or a resume too, where it showed under Waiting for work · nothing yet; a restart reads Codex's screen for them only until its first turn, so start-up words in a later diff keep no task from restarting
- The setup guide shows Codex on, and kebbie doctor names the plugin's folder, on a machine whose kebbie Codex plugin sits under kebbie@kebbie, where they showed Codex off and the plugin not installed; kebbie setup codex or kebbie update moves it to kebbie@kebbie-tasks
- A Codex start that meets Codex's welcome and sign-in screens, its notice of a new model or its update screen types nothing into them: the start says Codex asks you to sign in, which model to use or whether to update it first, as it does for the trust and hooks questions, and kebbie types the prompt once you have answered; the Codex sessions kebbie starts skip Codex's update check, so the update screen shows only where a managed config forces it
- A Codex task shows its name on the board and in the start's task line until Codex names its conversation, where it showed the command line the shell had put in the terminal's title
- A text kebbie could not deliver to a Codex task says Codex did not take it, where it named Claude
- kebbie's Codex plugin keeps working in a repo that ships a Claude marketplace named kebbie, kebbie's own among them: its id is now kebbie@kebbie-tasks, which Codex never installs a repo's copy over, kebbie update and kebbie setup codex move the plugin to it, and kebbie doctor names a copy Codex installed in its place
- A full review gives its verdict only when your kebbie review, the lead's or the keeper started its reviewer, with the nonce the rules check already carries, and runs the claude kebbie resolved for it; a review a task asks for, with kebbie review, its report or a test run, waits for the keeper to start it from its own environment
- kebbie setup codex --yes, and kebbie setup codex without a terminal, write kebbie's Codex plugin at once and say in one line what they wrote, then list what else the guide offers
- The setup guide's Agents step shows an out-of-date or disabled kebbie plugin in Claude Code or Codex as to update, with kebbie doctor's reason, where it called the plugin in place while doctor said it was out of date
- kebbie update and kebbie setup --yes bring an installed Codex plugin up to date with the new kebbie, so an update that changes its hooks no longer stops Codex tasks until you run kebbie setup codex; a machine without the plugin gets none

## 0.30.0

### What's new
- kebbie reads the comments colleagues leave on the commits it pushed, for seven days after each push: a read-only Claude judges each one as holds, opinion, praise or a question for you, and kebbie comments and kebbie comment show list them with a reply to send; comments.watch and comments.watch_days set it, and kebbie doctor says when a repo is not watched
- The board shows those comments under READY TO PUSH as their own cards, apart from Needs you and the lead: ⏎ opens one, a comment that holds reopens its task or starts a follow-up with the proposed fix and its commit message, and once the fix is pushed the card holds the reply, which y copies for you to post
- Z parks a task until tomorrow, Monday, a date or until you bring it back: it keeps working, leaves its group for one parked line that counts its questions and red CI, and stays out of the lead's summaries and your notifications until it comes back by itself; kebbie park and kebbie unpark do the same from a terminal
- The bugs to send fold into one row under Work that you select like a task, as the parked line does: ⏎ opens it and esc folds it back, and B is no longer a key
- Installed from herdr's plugin list, the board opens the setup guide each time it starts until kebbie's Claude Code plugin is installed, then restarts the lead on it
- Tasks work in steps by default: a task lists its steps with kebbie steps before its first line of code, one commit each, and you accept and push them one at a time; task.steps = false in config.toml or a repo's own file, or space on task.steps on the board's Settings page, turns it off
- herdr's tabs carry fixed names: kebbie in the lead Space and the task's name on a task's tab, set when kebbie creates or next starts them, instead of words from Claude's own title; a tab you renamed yourself keeps your name

### Fixed
- A step with a long title keeps its state in view on the board: the title is shortened with … instead, and the rules part reads in short, as committed · rules ?, rules ✓ or rules: 1 break
- A kebbie checkout you renamed or moved no longer leaves herdr starting nothing after a restart: the keeper links kebbie in herdr again from the current checkout and says so under messages, and kebbie doctor --fix does it when no keeper runs
- An install of kebbie's default branch from herdr's plugin list moves to the latest release on its next update, where the release channel refused it as a checkout you develop in
- Installing kebbie's default branch without Rust names the latest release and the install script as the way out, where it said vX.Y.Z
- An update of kebbie linked from your own checkout says it builds that checkout and how many commits it holds ahead of its upstream

## 0.29.1

### What's new

### Fixed
- A task the lead starts no longer pulls you into its Space; only a task you start takes focus
- Setup inside herdr ends in the lead's Space with the lead on the new kebbie, instead of
  opening a second board where setup ran
- The board no longer shows old text after its pane changes size while an ssh client is
  detached, and ctrl+l redraws every cell
- A stale lead says why it is stale, and a lead a restart cannot mend no longer offers L

## 0.29.0

### What's new
- kebbie setup opens your board when it is done: any key once it has applied, or after h has connected the hub
- kebbie:work-rules cuts a branch into steps in the order they go out: preparatory commits such as a migration, an interface, a move or a rename first, the change itself last and small, each one finished and pushable on its own
- With task.steps = true, off by default, a task lists its steps with kebbie steps before its first line of code, one commit each, keeps building the next one while you accept and push the last, and lists them again when you say the split is wrong; kebbie steps shows each one as planned, in progress, committed or accepted, with the pushed ones as a count
- kebbie accept <task> <sha> marks one of a task's commits as accepted by you, whatever the setting; it holds through a clean rebase or a reworded message, a changed line takes it back, and --withdraw takes it back too
- A task in steps shows them under its row on the board, with N to accept and the pushed ones as a count; the status line counts the commits waiting for your accept, and the push window marks accepted commits and opens on the last of them, so p pushes what you accepted while the task keeps working
- kebbie hub leave, and l in the board's H window, remove the machine from the hub, so the phone no longer lists it; a hub without that call still lists it, and kebbie says how its admin removes it
- The phone shows a task's steps under its row as the board does, each with its number, title and state, the pushed ones as a count, N to accept on the row and the total to accept at the top; a hub from before steps cannot open a board that carries them, so update a hub of your own first
- kebbie push --through, and p in the push window, leave the task's worktree alone: uncommitted work no longer stops it, the commits go out rebased in a worktree of kebbie's own, and the task's Claude rebases onto the new default branch when its work is committed; a commit it makes meanwhile no longer stops the push
- kebbie accepted <task> [--json] lists which of a task's commits are accepted, and a task may run it
- kebbie steps refuses a word it would take as an option; titles that start with - go after --
- The review split is kebbie's own view beside the diff: the task's steps, or its commits, as tabs with their marks, the selected one's commit message and its files on top, and nvim below showing the file you pick at once; ← → pick a step, ↑ ↓ a file, a accepts the step and moves on to the next one waiting, r marks a file read, / filters, m hides the message, and a click picks a step or a file
- tab moves between the steps, the files and the diff, and back; a review 120 columns wide or more puts the diff beside the view instead of under it
- The review split needs only nvim, not fzf
- A task whose review asked for tests goes back to the reviewer by itself once it records a passing run on the reviewed code
- A task in review says on its row who acts next: waits on the reviewer, or withdraw with v to push without a pass when it is yours
- The board's To review group is In review; board.groups takes "in-review", and "to-review" still works
- The reviewer reads every test run on the reviewed code, one line per command, and the older ones only when none ran on it
- The reviewer tells a broken test-run hook from no test run, and hears of commands that look like test runs but are not recorded
- A task that runs a command that looks like a test run, such as script/test or make test, but is not in task.test_commands names it on its row, with the hint to add it there so its runs are recorded
- Messages to the lead, kebbie's notices and desktop notifications name a task the way the board does, by its title with its name
- kebbie tells the lead live only what asks it to act; a passed review, a task started, pushed or closed, and a rebase or red CI the task resolves itself come with your next prompt to the lead, grouped by task
- While you talk with the lead, what asks it to act waits, and goes as one message grouped by task once you have been quiet for 3 minutes and the lead is idle; a prompt on a task's screen comes first, and in away mode nothing waits
- With [report] inbox_token set, an error signal at least three installs report for the same version, component and kind on one UTC day becomes one kebbie bug, such as "board.render: panic in 0.28.0 (4 installs on 2026-10-28)"; later days update that bug, and a new version opens its own
- A bug report or signal the inbox files adds one line under messages on the board, "New bug report: <title>", and leaves the lead alone
- Once a day kebbie sends its maintainers an install signal, so they can count active installs per version and OS: a random id made for this install, kebbie's version, the OS and architecture and the update channel, and nothing else; [report] signals = false turns it off with the error signals
- kebbie.dev's docs show a new release's pages within minutes of it being published
- The board's keys window (?) says how to report a problem with kebbie: B, or ask the lead

### Fixed
- Messages from kebbie show in Claude Code as one collapsed line from @kebbie that ctrl+o expands, instead of in full with Claude Code's peer warning
- y in the review split's diff copies to the clipboard, also over SSH from a machine with no clipboard tool, through the terminal
- c in the review split's diff comments on lines selected with a drag or V
- A question answered in your own words that asks the task something gets an answer from the task, as a new question on the board, before it writes code
- prefix+space in a lead Space whose board is not running starts the board, instead of leaving the lead
- A CI run that GitHub cancelled, because a deploy window refused it or a newer push superseded it, is no failure: the task stays under CI running, says which workflow waits for the next run on main, and closes once a later run on main that holds the pushed commit passes
- A task whose hooks run a kebbie binary that is gone restarts once it is idle, with settings that run kebbie, so its test runs and its vault gate work again; kebbie restart says what it restarted each session for
- A task that follows main is not rebased while it waits on you, with an open question or under Needs you, and follows again once it moves on
- A task's git push to a local path or a file:// URL, such as a repo a test makes, goes through; every network URL stays blocked
- A task's git push to a host:path URL such as git@github.com:acme/app.git fails at once, as every other network URL does, instead of starting ssh to a host named kebbie-blocks-push; kebbie update renews the block in every pooled worktree
- A listed test command run as ./script/test is recorded like script/test, and either spelling in task.test_commands matches both
- A task under CI running shows no review state: the code it was reviewed on is pushed
- The lead works without narrating its steps and ends each turn in three short parts: done, going on per task, and what needs you last with the exact answer it wants; a turn kebbie's messages started that needs nothing from you ends on one line
- kebbie update says Updated only for a plugin it updated, and names what became of one it did not, such as a Claude Code plugin that is not installed
- A task in review shows the review's state without rules not checked: the reviewer checks the rules, so a review under way or of the current code covers them
- A task's row shows your answer to its question, with the task working on it, until the task reports again, instead of the question it asked

## 0.28.1

### What's new
- The install command says what it installs, from which tag and commit, and what it changes, then asks once, and herdr's own listing shows only when the install fails
- kebbie setup has no hub step and never touches the hub; once it is done, a machine that is not connected gets a box offering the board on your phone: h connects now, any other key closes, and kebbie hub connect or H on the board connects later
- kebbie setup fixes the kebbie doctor warnings it can as rows of its review, shown before ⏎ like every other: kebbie's statusLine in Claude Code, the keeper and kebbie's herdr plugin; it ends with the doctor's count and next steps of a command and why each, and a warning that needs your choice is one line to kebbie doctor
- kebbie setup's review says each line in a few words, such as ~/.local/bin/kebbie or prefix+f added, herdr reloaded

### Fixed
- The hub's QR code is drawn in half blocks, which every font draws, in terminals other than Ghostty, kitty and WezTerm, and in octant blocks in Ghostty over SSH too; KEBBIE_QR=halfblock or octant picks one

## 0.28.0

### What's new
- kebbie setup is a guide of eight steps, one box each with the steps on the left: this machine's name, the agents (Claude Code, and Codex on or off), herdr's keys, the backlog's sources, the vault (a folder or an S3 bucket, configured for you with nothing to paste), the hub (straight to its connect card) and error signals, then a list of everything it will write or run, applied on ⏎ line by line, and next steps: what is still yours to do, each with its command or key, such as codex login, kebbie hub connect or a kebbie doctor fix; nothing is written before that, s skips a step, ← goes back and esc quits
- Run again, kebbie setup starts from this machine's values and marks what is already in place; kebbie setup --yes takes the defaults without asking, for scripts, and kebbie setup vault and kebbie setup codex open the guide at their step
- A herdr key you already use for something else is named in the setup guide, with a free key offered for kebbie's
- The board has a Settings page, on , or with kebbie config edit: every key kebbie config prints, by section, with its value, the layer it comes from and what it does, changed in place in config.toml or the repo's config with its comments and order kept, each write checked by kebbie doctor; a key from a linked team file asks whether to change the team's file or override it here
- On the Settings page, task.rules_skill and worktree.setup_skill offer a copy of kebbie's skill in ~/.claude/skills to edit as your own
- task.rules_skill and worktree.setup_skill take a Markdown file by its absolute or ~ path as well as a skill's name: the task gets the file's text in its role, and the reviewer and the rules check in their brief
- kebbie config and the configuration reference say plainly that restart.auto restarts only the sessions running older code than is installed, after an update or a new Claude Code, and that restart.idle_minutes is how long such a session waits to be idle and out of focus first
- kebbie setup adds three herdr keys, prefix+space, prefix+f and prefix+t; updating is U on the board, so prefix+u is no longer added
- Install with one command, curl -fsSL https://kebbie.dev/install.sh | sh: it checks what kebbie needs, installs the latest release in herdr and runs setup, with no herdr restart and no path to paste
- Connecting a machine from its link hands it your account's key: the machine checks the key against the link's secret, pins it, and seals every board to your account alone, so any phone or browser you sign in on with your passkey reads it, with nothing to add on the machine; kebbie hub status shows the account and its key's two fingerprints
- A machine approved by its typed code, or joined with a join code, posts no board, and the card, the H window, kebbie hub status and kebbie doctor say to connect it from the link: kebbie hub leave, then kebbie hub connect
- The board goes out at least once a minute while nothing changes, well within the day the hub keeps it, so an idle machine stays awake on the web and one really asleep shows as asleep
- Notifications through the hub are not available in this version: the phone shows the board when you open it, and notify.command still tells you on the machine when a task needs you
- Connecting a hub opens the browser only when you press ⏎, so you can read the card first
- The hub's QR code for connecting is drawn small in the terminal's own colours, in octant blocks in Ghostty, kitty and WezTerm, which the iPhone camera reads as a code and not as text, and in braille elsewhere; KEBBIE_QR=octant or braille picks one, and in a narrow pane the code sits under the text
- KEBBIE_HUB_DEV=1 points kebbie hub, the H window and the connect card at a dev hub on localhost:8470 (KEBBIE_HUB_DEV_URL for another address), with its own identity in hub-dev/, and says dev hub wherever it shows the hub; the real hub connection stays as it is, and the keeper posts the board to both
- kebbie setup and kebbie hub connect pick the hub with ↑ ↓ and ⏎; k and o still choose at once
- The hub window shows this machine on its hub with when the board last went out and where to sign in to see it; n renames this machine and l leaves the hub
- Connecting a hub shows a card: a QR code, the code to check on the page, the time left, and c to copy the link
- kebbie setup offers the hub as an optional last step, and asks which hub before the machine's name
- Signing in on the kebbie hub no longer asks for an account name
- kebbie speaks of the bugs in kebbie you ran into, plainly, on the board and in its messages
- The update shows each step as it goes, with a bar while kebbie builds or downloads, and ends with what's new and what restarts
- The line about the lead restarting on the new kebbie is short and always fits the board
- One bar shows the whole update, from fetching to the restarts, weighed by how long each step took last time, and the summary comes once it is full
- On the main channel kebbie calls itself kebbie main, in kebbie --version, kebbie doctor and the update window, and the board's ? window ends with which kebbie runs
- On the main channel the board's update notice counts the new commits, ↑ kebbie main, 3 new, and the update window names the commits it went from and to
- L on the board asks whether to start the lead fresh, on a new conversation from its role and the vault, or resume it; kebbie lead --restart --fresh starts it fresh, and the board says when the lead's context is nearly full

### Fixed
- The hub's QR code draws its light modules and quiet zone in the text colour and leaves the dark ones to the background, so on a dark theme it reads dark on light as a QR code should
- L on the board restarts the lead whenever you press it, and its result goes by like the board's other results
- prefix+space brings the board back in the lead Space after you quit it or closed its pane, and creates the lead Space with its board when there is none, never a second board
- The lead and other sessions restart onto each new kebbie again; a restart for an earlier update no longer holds them back
- The update's summary lists what was written since the kebbie you had, names only the bugs this update fixed, keeps its restarts up to date while open, and the board restarts itself onto the new kebbie when you close it
