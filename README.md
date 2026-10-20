![](assets/icon.svg)

# Nebula

Nebula is a local Rust CLI and Agent Skill for measuring communication and
execution efficiency one AI-agent turn at a time. It records timing, word
counts, estimates, outcomes, keywords, and numeric clarity scores in SQLite,
then summarizes them with `nebula report`.

Nebula does not persist prompt text, response text, rationale, source code,
credentials, or token content. Hook input is processed in memory and reduced to
numeric metrics. Historical import and model fitting follow the same rule;
optional import dumps contain metadata and a scoring scaffold, never message
text.

## Install

Download the installer before running it:

```sh
curl --proto '=https' --tlsv1.2 -fsSLO \
  https://raw.githubusercontent.com/ennerperez/nebula/master/install.sh
sh install.sh
```

The installer selects the matching GitHub release, verifies its SHA-256
checksum, installs `nebula` to `~/.local/bin`, and installs the bundled skill
for detected AI agents. It supports Codex, Claude Code, Gemini CLI, OpenCode,
and the generic Agent Skills directory. When Codex or Claude Code is selected
or detected, it also configures Nebula's lifecycle hooks automatically.
Restart the agent after first installation so the hooks load. Codex also
requires reviewing and trusting the installed hooks with `/hooks`.

Select targets or paths explicitly when needed:

```sh
sh install.sh --agent codex
sh install.sh --agent claude --agent gemini
```

Or use custom paths:

```sh
sh install.sh --bin-dir "$HOME/.local/bin" --skill-dir "$HOME/.claude-custom/skills"
CLAUDE_CONFIG_DIR="$HOME/.claude-custom" \
  sh install.sh --agent claude --bin-dir "$HOME/.local/bin"
```

See [CLI guide](docs/CLI.md#installation) for supported platforms, every
installer option, source builds, updates, and removal.

## Quick start

```sh
nebula --version
nebula metrics list
nebula report --days 7 --lang en
nebula tips --both --share 0.3
nebula report --days 30 --format html
```

Each installed agent skill stores data below its own agent home, such as
`~/.codex/nebula` or `~/.claude/nebula`. Set `NEBULA_HOME` explicitly when
multiple agents should use one shared database. Standalone CLI calls without
agent context use `~/.agents/nebula`.

The Agent Skill defines the per-turn workflow: a host hook opens the turn,
the agent records its estimate and clarity scores with `log-start`, and it
checks `tips` before closing its model-side work with `log-end`. Live tips are
agent-written, limited to 90 characters, and displayed only when the current
prompt scores worse than its same-session baseline. The host stop hook records
elapsed time and response measurements. Installing the skill does not modify
hook configuration for generic hosts; the release installer configures Codex
and Claude Code hooks automatically.

## Commands

Implemented commands:

- `configure codex` safely merges lifecycle hooks into Codex `hooks.json`.
- `configure claude` safely merges lifecycle hooks into Claude Code settings.
- `data clear --yes` permanently removes Nebula-managed local data without
  touching external transcripts, the CLI, or installed skills.
- `hook <EVENT>` processes `user-prompt-submit`, `stop`, or `notification`
  events from JSON on stdin.
- `log-start` records an estimate, keywords, task metadata, and user-to-agent
  clarity scores for an open turn.
- `log-end` records agent-to-user clarity scores and the outcome for an open
  turn.
- `report` builds one structured report for Markdown, standalone HTML,
  structured JSON, tidy CSV, or PDF. Agent-written recommendations enter through
  `--recs`; `--format` is preferred and `--export` remains compatible.
- `tips` prints recent-window, earlier, delta, and last-turn measurements for
  live agent-written guidance; `--both`, `--all`, and `--session` select scope.
- `estimate` predicts duration from turns with matching keywords.
- `calibrate` aligns heuristic scores to reviewed model scores.
- `fit` learns transcript-derived heuristic models with cross-validation while
  keeping transcript text in memory.
- `import` reconstructs metrics from supported local transcript histories;
  `import-scores` adds reviewed model scores to imported turns.
- `metrics list` lists metric definitions.
- `metrics add` adds a custom metric definition.

User-facing output is available in English and Spanish. Pass `--lang en` or
`--lang es`; otherwise Nebula uses the active session language,
`NEBULA_LANG`, then English.

See the [complete CLI reference](docs/CLI.md#command-reference) for options,
examples, output, errors, storage, and lifecycle details.

## Development

Requires the stable Rust toolchain:

```sh
cargo build --release --locked --bin nebula
cargo test --locked
```

The release binary is written to `target/release/nebula`. Stack and portability
notes live in [docs/STACK.md](docs/STACK.md).

## License

[MIT](LICENSE)
