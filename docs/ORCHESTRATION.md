# Coding agent orchestration

Comrade coordinates installed coding CLIs: OpenCode, Codex, Claude Code, GitHub
Copilot CLI, Qwen Code and Hermes. There is no Docker requirement.

Install and log in to each agent you want to use, and ensure its executable is
on the PATH available to Comrade. Choose enabled agents and a default in Settings.
No separate provider-key setup is required in Comrade; each CLI uses its own
configuration and login. Missing login or native permission errors appear in logs.

Open **Comrade Orch** below **New chat** in the sidebar. Use **Add project**
to choose a directory with the native folder picker. Saved projects stay in the
sidebar; selecting one shows its task plans and run history. Enter a goal,
then assign tasks to agents. Optional models override each CLI's default. Add
dependencies when a task needs another task's edits or results. Tasks share the
project directory, so order tasks that edit the same files. Run options set up to
three concurrent jobs and a 30–1800 second deadline per job.

**Start run** launches the plan. **Let Comrade plan** sends your goal to Comrade
with the selected project as separate context. Comrade uses installed, enabled
agents and presents the concrete plan through its tool approval dialog. Recent
chat turns and project context are retained across follow-ups and app restarts,
so replies such as "yes" continue the agreed task. Selecting a different project
starts a separate project conversation. The workspace shows persistent run history, individual
statuses, streamed stdout/stderr, errors, and Stop controls for jobs or whole runs.
Failed or stopped dependencies block downstream jobs. Successful process exit
means the agent finished; read its output to see which checks actually ran.

Agents edit files directly. Stopping a run stops its processes, and does not undo
edits already made. Comrade does not add a host isolation boundary or automatically
approve every native tool. Codex uses workspace-write, Claude accepts file edits;
other native permission settings stay with the CLI. Noninteractive agents cannot
answer permission prompts through this workspace. Broader isolation can be added later.

Run state lives in `<COMRADE_HOME>/orchestration/runs.json`. Logs retain the latest
32 KB per job, with a total output limit and redaction of known provider keys from
the environment. Comrade restart marks unfinished recorded jobs interrupted and
never relaunches them or kills a persisted process id. On Unix each worker owns a
process group, and cancellation/deadlines stop the group; Windows uses taskkill /T.

The core validates task ids, agent/model choices and acyclic dependencies before
launching. Prompts and models are passed as separate process arguments. The tools
are `coding.executeTask`, `coding.startPlan`, `coding.runtimeStatus`,
`coding.runStatus` and `coding.cancelRun`.

Adapter command references: [OpenCode run](https://opencode.ai/v2/docs/cli/commands/),
[Hermes CLI](https://hermes-agent.nousresearch.com/docs/user-guide/cli/), and
[Qwen headless mode](https://qwenlm.github.io/qwen-code-docs/en/users/features/headless/).

Project folders are remembered locally. Switching to chat or another project does
not stop active jobs. Use Stop job or Stop run to cancel work. The folder picker
uses the [Tauri dialog plugin](https://v2.tauri.app/plugin/dialog/).
