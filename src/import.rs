use crate::{
    cli::TranscriptHost,
    db::{add_keywords, put_metric},
    i18n::Localizer,
    learning::learned_heuristic,
    scoring::{
        asks_question, fallback_keywords, looks_like_correction, parse_scores, unmarked_questions,
        word_count,
    },
};
use anyhow::{Context, Result, anyhow};
use chrono::{DateTime, FixedOffset};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet, HashSet},
    env,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

static TICKET: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Z][A-Z0-9]+-\d+").unwrap());

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug)]
struct Message {
    timestamp: DateTime<FixedOffset>,
    timestamp_text: String,
    role: Role,
    text: String,
    request: Option<String>,
    agent_spawn: bool,
}

#[derive(Default)]
struct Transcript {
    session: String,
    codex: bool,
    started_at: Option<String>,
    ended_at: Option<String>,
    cwd: Option<PathBuf>,
    repo_url: Option<String>,
    branch: Option<String>,
    model: Option<String>,
    messages: Vec<Message>,
    request_starts: Vec<(DateTime<FixedOffset>, String)>,
}

#[derive(Default, Debug, Eq, PartialEq)]
pub(crate) struct ImportSummary {
    pub(crate) sessions: usize,
    pub(crate) turns: usize,
    pub(crate) skipped: usize,
}

#[derive(Serialize)]
struct SafeDump {
    schema: &'static str,
    privacy: &'static str,
    turns: Vec<SafeTurn>,
}

#[derive(Serialize)]
struct SafeTurn {
    turn_id: i64,
    session_id: String,
    seq: usize,
    started_at: String,
    ended_at: String,
    previous_answer_asked_question: bool,
    prompt_words: usize,
    answer_words: usize,
    server_requests: usize,
    agents: usize,
    keywords: Vec<String>,
    u2a_heuristic: BTreeMap<String, f64>,
    a2u_heuristic: BTreeMap<String, f64>,
}

pub(crate) fn import_sessions(
    connection: &Connection,
    sessions: usize,
    exclude: &[String],
    dump: Option<&Path>,
    host: Option<TranscriptHost>,
    localizer: &Localizer,
) -> Result<()> {
    let roots = transcript_roots(host, localizer)?;
    let summary = import_with_roots(connection, sessions, exclude, dump, localizer, &roots)?;
    println!(
        "{}",
        localizer.format(
            "import.total",
            &[
                ("turns", summary.turns.to_string()),
                ("sessions", summary.sessions.to_string()),
                ("skipped", summary.skipped.to_string()),
            ],
        )
    );
    Ok(())
}

fn import_with_roots(
    connection: &Connection,
    limit: usize,
    exclude: &[String],
    dump: Option<&Path>,
    localizer: &Localizer,
    roots: &[PathBuf],
) -> Result<ImportSummary> {
    let excluded = exclude.iter().cloned().collect::<HashSet<_>>();
    let mut candidates = transcript_files(roots)?;
    candidates.sort_by_key(|path| {
        std::cmp::Reverse(
            fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .ok(),
        )
    });

    let mut summary = ImportSummary::default();
    let mut safe_turns = Vec::new();
    let mut considered = 0;
    for path in candidates {
        let transcript = read_transcript(&path)?;
        if transcript.session.is_empty() || excluded.contains(&transcript.session) {
            continue;
        }
        considered += 1;
        if considered > limit {
            break;
        }
        if connection
            .query_row(
                "SELECT 1 FROM sessions WHERE id=?1",
                [&transcript.session],
                |_| Ok(()),
            )
            .optional()?
            .is_some()
        {
            summary.skipped += 1;
            println!(
                "{}",
                localizer.format(
                    "import.skip_imported",
                    &[("session", short(&transcript.session))]
                )
            );
            continue;
        }
        let transaction = connection.unchecked_transaction()?;
        let imported = import_transcript(&transaction, transcript, &mut safe_turns)?;
        transaction.commit()?;
        if imported > 0 {
            summary.sessions += 1;
            summary.turns += imported;
        }
    }

    if let Some(path) = dump {
        let payload = SafeDump {
            schema: "nebula.safe-import-dump.v1",
            privacy: "No transcript text is included. Only numeric metrics, categorical metadata, derived keywords, and heuristic scores are exported.",
            turns: safe_turns,
        };
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, serde_json::to_vec_pretty(&payload)?)?;
        println!(
            "{}",
            localizer.format(
                "import.dump_saved",
                &[
                    ("turns", payload.turns.len().to_string()),
                    ("path", path.display().to_string()),
                ],
            )
        );
    }
    Ok(summary)
}

fn import_transcript(
    connection: &Connection,
    mut transcript: Transcript,
    dump: &mut Vec<SafeTurn>,
) -> Result<usize> {
    transcript.messages.sort_by_key(|message| message.timestamp);
    for message in transcript
        .messages
        .iter_mut()
        .filter(|message| message.role == Role::User)
    {
        message.text = prompt_text(&message.text).unwrap_or_default();
    }
    let prompts = transcript
        .messages
        .iter()
        .enumerate()
        .filter(|(_, message)| message.role == Role::User && is_real_prompt(&message.text))
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if prompts.is_empty() {
        return Ok(0);
    }

    let started_at = transcript
        .started_at
        .clone()
        .unwrap_or_else(|| transcript.messages.first().unwrap().timestamp_text.clone());
    let ended_at = transcript
        .ended_at
        .clone()
        .unwrap_or_else(|| transcript.messages.last().unwrap().timestamp_text.clone());
    let repo = transcript.repo_url.clone().or_else(|| {
        transcript
            .cwd
            .as_deref()
            .and_then(|cwd| git(cwd, &["remote", "get-url", "origin"]))
            .and_then(|url| repo_path(&url))
    });
    let branch = transcript.branch.clone().or_else(|| {
        transcript
            .cwd
            .as_deref()
            .and_then(|cwd| git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]))
    });
    let ticket = branch
        .as_deref()
        .and_then(|branch| TICKET.find(branch))
        .map(|value| value.as_str().to_owned());
    connection.execute(
        "INSERT INTO sessions(id,started_at,ended_at,repo_url,branch,ticket,model)
         VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            transcript.session,
            started_at,
            ended_at,
            repo,
            branch,
            ticket,
            transcript.model
        ],
    )?;

    let mut imported = 0;
    let mut previous_answer = String::new();
    let mut previous_turn = None;
    for (position, prompt_index) in prompts.iter().copied().enumerate() {
        let next = prompts
            .get(position + 1)
            .copied()
            .unwrap_or(transcript.messages.len());
        let prompt = &transcript.messages[prompt_index];
        let window = &transcript.messages[prompt_index + 1..next];
        let assistants = window
            .iter()
            .filter(|message| message.role == Role::Assistant)
            .collect::<Vec<_>>();
        if assistants.is_empty() {
            continue;
        }
        let last = assistants.last().expect("non-empty assistant messages");
        let answer = assistants
            .iter()
            .map(|message| message.text.as_str())
            .filter(|text| !text.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        if answer.trim().is_empty() {
            continue;
        }
        let requests =
            requests_in_window(&transcript, prompt.timestamp, last.timestamp, &assistants);
        let agents = assistants
            .iter()
            .filter(|message| message.agent_spawn)
            .count();
        let sequence = imported + 1;
        connection.execute(
            "INSERT INTO turns(session_id,seq,started_at,ended_at,outcome)
             VALUES(?1,?2,?3,?4,'completed')",
            params![
                transcript.session,
                sequence,
                prompt.timestamp_text,
                last.timestamp_text
            ],
        )?;
        let turn = connection.last_insert_rowid();
        let duration = (last.timestamp - prompt.timestamp).num_milliseconds() as f64 / 1000.;
        let replies = asks_question(&previous_answer);
        let u2a = learned_heuristic(connection, &prompt.text, replies, "u2a")?;
        let a2u = learned_heuristic(connection, &answer, asks_question(&prompt.text), "a2u")?;
        put_metric(
            connection,
            turn,
            "real_seconds",
            round1(duration.max(0.)),
            "transcript",
            None,
        )?;
        put_metric(
            connection,
            turn,
            "words_in",
            word_count(&prompt.text) as f64,
            "transcript",
            None,
        )?;
        put_metric(
            connection,
            turn,
            "words_out",
            word_count(&answer) as f64,
            "transcript",
            None,
        )?;
        put_metric(
            connection,
            turn,
            "server_requests",
            requests as f64,
            "transcript",
            None,
        )?;
        put_metric(
            connection,
            turn,
            "agents",
            agents as f64,
            "transcript",
            None,
        )?;
        for (key, value) in &u2a {
            put_metric(connection, turn, key, *value, "heuristic", Some("u2a"))?;
        }
        for (key, value) in &a2u {
            put_metric(connection, turn, key, *value, "heuristic", Some("a2u"))?;
        }
        put_metric(
            connection,
            turn,
            "unmarked_questions",
            unmarked_questions(&prompt.text) as f64,
            "heuristic",
            Some("u2a"),
        )?;
        put_metric(
            connection,
            turn,
            "unmarked_questions",
            unmarked_questions(&answer) as f64,
            "heuristic",
            Some("a2u"),
        )?;
        if looks_like_correction(&prompt.text, replies)
            && let Some(previous) = previous_turn
        {
            put_metric(connection, previous, "corrections", 1., "heuristic", None)?;
        }
        let keywords = fallback_keywords(&prompt.text, 5);
        add_keywords(connection, turn, &keywords.join(","))?;
        dump.push(SafeTurn {
            turn_id: turn,
            session_id: transcript.session.clone(),
            seq: sequence,
            started_at: prompt.timestamp_text.clone(),
            ended_at: last.timestamp_text.clone(),
            previous_answer_asked_question: replies,
            prompt_words: word_count(&prompt.text),
            answer_words: word_count(&answer),
            server_requests: requests,
            agents,
            keywords,
            u2a_heuristic: u2a,
            a2u_heuristic: a2u,
        });
        previous_answer = answer;
        previous_turn = Some(turn);
        imported += 1;
    }
    if imported == 0 {
        connection.execute("DELETE FROM sessions WHERE id=?1", [&transcript.session])?;
    }
    Ok(imported)
}

fn requests_in_window(
    transcript: &Transcript,
    start: DateTime<FixedOffset>,
    end: DateTime<FixedOffset>,
    assistants: &[&Message],
) -> usize {
    let started = transcript
        .request_starts
        .iter()
        .filter(|(timestamp, _)| *timestamp >= start && *timestamp <= end)
        .map(|(_, id)| id)
        .collect::<BTreeSet<_>>();
    if transcript.codex && !started.is_empty() {
        return started.len();
    }
    let mut requests = assistants
        .iter()
        .filter_map(|message| message.request.as_ref())
        .collect::<BTreeSet<_>>();
    requests.extend(started);
    requests.len().max(1)
}

pub(crate) fn import_scores(
    connection: &Connection,
    path: &Path,
    localizer: &Localizer,
) -> Result<()> {
    let contents = fs::read_to_string(path).with_context(|| {
        localizer.format("import.read_error", &[("path", path.display().to_string())])
    })?;
    let rows: Vec<Value> = serde_json::from_str(&contents).map_err(|error| {
        anyhow!(localizer.format("import.invalid_file", &[("error", error.to_string())]))
    })?;
    let transaction = connection.unchecked_transaction()?;
    let (scored, skipped) = apply_scores(&transaction, &rows, localizer)?;
    transaction.commit()?;
    println!(
        "{}",
        localizer.format(
            "import.scored",
            &[
                ("scored", scored.to_string()),
                ("skipped", skipped.to_string())
            ]
        )
    );
    Ok(())
}

fn apply_scores(
    connection: &Connection,
    rows: &[Value],
    localizer: &Localizer,
) -> Result<(usize, usize)> {
    let mut scored = 0;
    let mut skipped = 0;
    for (index, row) in rows.iter().enumerate() {
        let turn = row.get("turn_id").and_then(Value::as_i64).ok_or_else(|| {
            anyhow!(localizer.format("import.invalid_turn", &[("row", (index + 1).to_string())]))
        })?;
        if connection
            .query_row("SELECT 1 FROM turns WHERE id=?1", [turn], |_| Ok(()))
            .optional()?
            .is_none()
        {
            skipped += 1;
            println!(
                "{}",
                localizer.format("import.skip_unknown", &[("turn", turn.to_string())])
            );
            continue;
        }
        for direction in ["u2a", "a2u"] {
            if let Some(value) = row.get(direction).filter(|value| !value.is_null()) {
                let parsed = parse_scores(&value.to_string(), direction, localizer)?;
                for (key, score) in parsed {
                    put_metric(connection, turn, &key, score, "model", Some(direction))?;
                }
            }
        }
        if let Some(keywords) = row.get("keywords") {
            let words = keywords.as_array().ok_or_else(|| {
                anyhow!(localizer.format(
                    "import.invalid_keywords",
                    &[("row", (index + 1).to_string())]
                ))
            })?;
            let words = words
                .iter()
                .map(|word| {
                    word.as_str().ok_or_else(|| {
                        anyhow!(localizer.format(
                            "import.invalid_keywords",
                            &[("row", (index + 1).to_string())]
                        ))
                    })
                })
                .collect::<Result<Vec<_>>>()?;
            add_keywords(connection, turn, &words.join(","))?;
        }
        if let Some(task_type) = row.get("task_type").filter(|value| !value.is_null()) {
            let task_type = task_type.as_str().ok_or_else(|| {
                anyhow!(localizer.format(
                    "import.invalid_task_type",
                    &[("row", (index + 1).to_string())]
                ))
            })?;
            connection.execute(
                "UPDATE turns SET task_type=?1 WHERE id=?2",
                params![task_type, turn],
            )?;
        }
        if let Some(clarifying) = row
            .get("clarifying_questions")
            .filter(|value| !value.is_null())
        {
            let value = clarifying
                .as_f64()
                .filter(|value| *value >= 0.)
                .ok_or_else(|| {
                    anyhow!(localizer.format(
                        "import.invalid_clarifying",
                        &[("row", (index + 1).to_string())]
                    ))
                })?;
            put_metric(
                connection,
                turn,
                "clarifying_questions",
                value,
                "model",
                None,
            )?;
        }
        if row
            .get("corrects_previous")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            && let Some(previous) = connection
                .query_row(
                    "SELECT p.id FROM turns t JOIN turns p
                     ON p.session_id=t.session_id AND p.seq=t.seq-1 WHERE t.id=?1",
                    [turn],
                    |result| result.get::<_, i64>(0),
                )
                .optional()?
        {
            put_metric(connection, previous, "corrections", 1., "model", None)?;
        }
        scored += 1;
    }
    Ok((scored, skipped))
}

fn transcript_roots(host: Option<TranscriptHost>, localizer: &Localizer) -> Result<Vec<PathBuf>> {
    if let Some(value) = env::var_os("NEBULA_TRANSCRIPTS") {
        return Ok(env::split_paths(&value).collect());
    }
    let Some(home) = dirs::home_dir() else {
        return Err(anyhow!(localizer.text("import.ambiguous_host")));
    };
    transcript_roots_for(
        host,
        env::var_os("CLAUDE_CONFIG_DIR"),
        env::var_os("CLAUDECODE").is_some(),
        env::var_os("CODEX_HOME"),
        env::var_os("CODEX_THREAD_ID").is_some() || env::var_os("CODEX_SESSION_ID").is_some(),
        &home,
    )
    .ok_or_else(|| anyhow!(localizer.text("import.ambiguous_host")))
}

fn transcript_roots_for(
    host: Option<TranscriptHost>,
    claude_config: Option<OsString>,
    claude_active: bool,
    codex_home: Option<OsString>,
    codex_active: bool,
    home: &Path,
) -> Option<Vec<PathBuf>> {
    if let Some(host) = host {
        return Some(match host {
            TranscriptHost::Claude => vec![
                claude_config
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".claude"))
                    .join("projects"),
            ],
            TranscriptHost::Codex => vec![
                codex_home
                    .map(PathBuf::from)
                    .unwrap_or_else(|| home.join(".codex"))
                    .join("sessions"),
            ],
        });
    }
    if claude_active && codex_active {
        return None;
    }
    if codex_active {
        return Some(vec![
            codex_home
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".codex"))
                .join("sessions"),
        ]);
    }
    if claude_active {
        return Some(vec![
            claude_config
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join(".claude"))
                .join("projects"),
        ]);
    }
    match (claude_config, codex_home) {
        (Some(path), None) => Some(vec![PathBuf::from(path).join("projects")]),
        (None, Some(path)) => Some(vec![PathBuf::from(path).join("sessions")]),
        (Some(_), Some(_)) | (None, None) => None,
    }
}

fn transcript_files(roots: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let mut output = Vec::new();
    let mut stack = roots.to_vec();
    while let Some(path) = stack.pop() {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_file() {
            if path
                .extension()
                .is_some_and(|extension| extension == "jsonl")
                && !path
                    .components()
                    .any(|component| component.as_os_str() == "subagents")
            {
                output.push(path);
            }
        } else if let Ok(entries) = fs::read_dir(&path) {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    Ok(output)
}

fn read_transcript(path: &Path) -> Result<Transcript> {
    let contents = fs::read_to_string(path)?;
    let entries = contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    let codex = entries
        .iter()
        .any(|entry| entry.get("type").and_then(Value::as_str) == Some("session_meta"));
    let mut transcript = if codex {
        parse_codex(path, &entries)
    } else {
        parse_claude(path, &entries)
    };
    let mut times = entries.iter().filter_map(entry_time).collect::<Vec<_>>();
    times.sort_by_key(|(timestamp, _)| *timestamp);
    transcript.started_at = times.first().map(|(_, text)| text.clone());
    transcript.ended_at = times.last().map(|(_, text)| text.clone());
    if !codex {
        let mut directory = path.with_extension("");
        directory.push("subagents");
        if let Ok(entries) = fs::read_dir(directory) {
            for subagent in entries.flatten().map(|entry| entry.path()) {
                if subagent
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
                {
                    let subentries = fs::read_to_string(subagent)
                        .unwrap_or_default()
                        .lines()
                        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
                        .collect::<Vec<_>>();
                    transcript
                        .request_starts
                        .extend(subentries.iter().filter_map(|entry| {
                            if entry.get("type").and_then(Value::as_str) != Some("assistant") {
                                return None;
                            }
                            let (timestamp, timestamp_text) = entry_time(entry)?;
                            let request = entry
                                .get("requestId")
                                .and_then(Value::as_str)
                                .or_else(|| entry.pointer("/message/id").and_then(Value::as_str))
                                .unwrap_or(&timestamp_text)
                                .to_owned();
                            Some((timestamp, request))
                        }));
                }
            }
        }
    }
    Ok(transcript)
}

fn parse_claude(path: &Path, entries: &[Value]) -> Transcript {
    let mut output = Transcript {
        session: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        codex: false,
        ..Transcript::default()
    };
    for entry in entries {
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        output.cwd = output
            .cwd
            .take()
            .or_else(|| entry.get("cwd").and_then(Value::as_str).map(PathBuf::from));
        output.branch = entry
            .get("gitBranch")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .or(output.branch);
        let Some((timestamp, timestamp_text)) = entry_time(entry) else {
            continue;
        };
        let kind = entry.get("type").and_then(Value::as_str);
        let Some(role) = (match kind {
            Some("user") => Some(Role::User),
            Some("assistant") => Some(Role::Assistant),
            _ => None,
        }) else {
            continue;
        };
        if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let message = entry.get("message").unwrap_or(&Value::Null);
        let content = message.get("content").unwrap_or(&Value::Null);
        if role == Role::User
            && content.as_array().is_some_and(|blocks| {
                blocks
                    .iter()
                    .any(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
            })
        {
            continue;
        }
        let text = content_text(content);
        let agent_spawn = content.as_array().is_some_and(|blocks| {
            blocks.iter().any(|block| {
                block.get("type").and_then(Value::as_str) == Some("tool_use")
                    && matches!(
                        block.get("name").and_then(Value::as_str),
                        Some("Agent" | "Task")
                    )
            })
        });
        let model = message
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if model.is_some() {
            output.model = model.clone();
        }
        output.messages.push(Message {
            timestamp,
            timestamp_text,
            role,
            text,
            request: entry
                .get("requestId")
                .and_then(Value::as_str)
                .or_else(|| message.get("id").and_then(Value::as_str))
                .map(str::to_owned),
            agent_spawn,
        });
    }
    output
}

fn parse_codex(path: &Path, entries: &[Value]) -> Transcript {
    let mut output = Transcript {
        session: path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        codex: true,
        ..Transcript::default()
    };
    for entry in entries {
        let Some((timestamp, timestamp_text)) = entry_time(entry) else {
            continue;
        };
        let payload = entry.get("payload").unwrap_or(&Value::Null);
        match entry.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                output.session = payload
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| payload.get("session_id").and_then(Value::as_str))
                    .unwrap_or(&output.session)
                    .to_owned();
                output.cwd = payload
                    .get("cwd")
                    .and_then(Value::as_str)
                    .map(PathBuf::from);
                output.branch = payload
                    .pointer("/git/branch")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                output.repo_url = payload
                    .pointer("/git/repository_url")
                    .and_then(Value::as_str)
                    .and_then(repo_path);
            }
            Some("turn_context") => {
                if let Some(model) = payload.get("model").and_then(Value::as_str) {
                    output.model = Some(model.to_owned());
                }
            }
            Some("event_msg")
                if payload.get("type").and_then(Value::as_str) == Some("task_started") =>
            {
                let id = payload
                    .get("turn_id")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .unwrap_or_else(|| timestamp_text.clone());
                output.request_starts.push((timestamp, id));
            }
            Some("response_item")
                if payload.get("type").and_then(Value::as_str) == Some("message") =>
            {
                let role = match payload.get("role").and_then(Value::as_str) {
                    Some("user") => Role::User,
                    Some("assistant") => Role::Assistant,
                    _ => continue,
                };
                output.messages.push(Message {
                    timestamp,
                    timestamp_text,
                    role,
                    text: content_text(payload.get("content").unwrap_or(&Value::Null)),
                    request: payload.get("id").and_then(Value::as_str).map(str::to_owned),
                    agent_spawn: false,
                });
            }
            Some("response_item")
                if payload.get("type").and_then(Value::as_str) == Some("function_call") =>
            {
                if payload
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.contains("spawn_agent"))
                {
                    output.messages.push(Message {
                        timestamp,
                        timestamp_text,
                        role: Role::Assistant,
                        text: String::new(),
                        request: payload
                            .get("call_id")
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                        agent_spawn: true,
                    });
                }
            }
            _ => {}
        }
    }
    output
}

fn entry_time(entry: &Value) -> Option<(DateTime<FixedOffset>, String)> {
    let text = entry.get("timestamp").and_then(Value::as_str)?;
    Some((DateTime::parse_from_rfc3339(text).ok()?, text.to_owned()))
}

fn content_text(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                if matches!(
                    block.get("type").and_then(Value::as_str),
                    Some("text" | "input_text" | "output_text")
                ) {
                    block.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn is_real_prompt(text: &str) -> bool {
    prompt_text(text).is_some()
}

fn prompt_text(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty()
        || text.starts_with("<agent-message")
        || text.starts_with("<task-notification")
        || text.starts_with("Caveat:")
        || text.starts_with("This session is being continued from a previous conversation")
    {
        return None;
    }
    if text.starts_with("<command-message>") || text.starts_with("<command-name>") {
        let name = between(text, "<command-name>", "</command-name>").unwrap_or("");
        let args = between(text, "<command-args>", "</command-args>").unwrap_or("");
        let command = format!("{} {}", name.trim(), args.trim()).trim().to_owned();
        return (!command.is_empty()).then_some(command);
    }
    if text.starts_with('<') {
        return None;
    }
    Some(text.to_owned())
}

fn between<'a>(input: &'a str, start: &str, end: &str) -> Option<&'a str> {
    let value = input.split_once(start)?.1;
    Some(value.split_once(end)?.0)
}

fn git(cwd: &Path, args: &[&str]) -> Option<String> {
    Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|output| output.trim().to_owned())
        .filter(|output| !output.is_empty())
}

fn repo_path(url: &str) -> Option<String> {
    let trimmed = url.trim_end_matches(".git").trim_end_matches('/');
    let separator = trimmed.rfind([':', '/'])?;
    let owner = trimmed[..separator]
        .rfind([':', '/'])
        .map_or(0, |index| index + 1);
    Some(trimmed[owner..].to_owned())
}

fn short(session: &str) -> String {
    session.chars().take(8).collect()
}

fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, i18n::Lang};
    use serde_json::json;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);

    fn fixture() -> PathBuf {
        let path = env::temp_dir().join(format!(
            "nebula-import-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        db::initialize(&connection).unwrap();
        connection
    }

    fn english() -> Localizer {
        Localizer::load(Lang::En).unwrap()
    }

    #[test]
    fn transcript_roots_scope_discovery_to_active_host_and_profile() {
        let home = Path::new("/home/test");
        assert_eq!(
            transcript_roots_for(
                None,
                Some("/profiles/alt".into()),
                true,
                Some("/profiles/codex".into()),
                true,
                home,
            ),
            None
        );
        assert_eq!(
            transcript_roots_for(None, Some("/profiles/alt".into()), true, None, false, home,),
            Some(vec![PathBuf::from("/profiles/alt/projects")])
        );
        assert_eq!(
            transcript_roots_for(None, None, true, None, false, home),
            Some(vec![PathBuf::from("/home/test/.claude/projects")])
        );
        assert_eq!(
            transcript_roots_for(
                None,
                None,
                false,
                Some("/profiles/codex".into()),
                true,
                home,
            ),
            Some(vec![PathBuf::from("/profiles/codex/sessions")])
        );
        assert_eq!(
            transcript_roots_for(None, None, false, None, false, home),
            None
        );
        assert_eq!(
            transcript_roots_for(
                Some(TranscriptHost::Claude),
                Some("/profiles/alt".into()),
                false,
                Some("/profiles/codex".into()),
                true,
                home,
            ),
            Some(vec![PathBuf::from("/profiles/alt/projects")])
        );
    }

    fn scores(value: f64) -> Value {
        Value::Object(
            crate::scoring::KEYS
                .into_iter()
                .map(|key| (key.to_owned(), json!(value)))
                .collect(),
        )
    }

    fn write_claude(root: &Path, session: &str, second_prompt: &str) -> PathBuf {
        let path = root.join(format!("{session}.jsonl"));
        let lines = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"user","cwd":root,"gitBranch":"feature/NEB-7-import","message":{"content":"Please fix private widget src/a.rs"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"assistant","requestId":"r1","message":{"model":"claude-test","content":[{"type":"tool_use","name":"Task"},{"type":"text","text":"Initial result"}]}}),
            json!({"timestamp":"2026-01-01T00:00:02Z","type":"user","message":{"content":second_prompt}}),
            json!({"timestamp":"2026-01-01T00:00:04Z","type":"assistant","requestId":"r2","message":{"model":"claude-test","content":[{"type":"text","text":"Done privately"}]}}),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        path
    }

    #[test]
    fn import_is_idempotent_honors_exclusions_tracks_corrections_and_keeps_text_private() {
        let root = fixture();
        let main = write_claude(&root, "session-one", "No, that is wrong");
        let mut subagents = main.with_extension("");
        subagents.push("subagents");
        fs::create_dir_all(&subagents).unwrap();
        fs::write(
            subagents.join("agent.jsonl"),
            json!({"timestamp":"2026-01-01T00:00:00.500Z","type":"assistant","isSidechain":true,"requestId":"sub-r1","message":{"content":[{"type":"text","text":"sensitive subagent output"}]}}).to_string(),
        )
        .unwrap();
        write_claude(&root, "session-two", "Continue safely");
        let connection = database();
        let dump = root.join("safe.json");
        let first = import_with_roots(
            &connection,
            10,
            &["session-two".into()],
            Some(&dump),
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(
            first,
            ImportSummary {
                sessions: 1,
                turns: 2,
                skipped: 0
            }
        );
        let second = import_with_roots(
            &connection,
            10,
            &["session-two".into()],
            None,
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(second.skipped, 1);
        let corrections: f64 = connection.query_row("SELECT value FROM metric_values v JOIN metric_definitions d ON d.id=v.metric_id WHERE d.key='corrections'", [], |row| row.get(0)).unwrap();
        assert_eq!(corrections, 1.);
        let usage: (f64, f64) = connection.query_row("SELECT MAX(CASE WHEN d.key='server_requests' THEN v.value END),MAX(CASE WHEN d.key='agents' THEN v.value END) FROM metric_values v JOIN metric_definitions d ON d.id=v.metric_id JOIN turns t ON t.id=v.turn_id WHERE t.session_id='session-one' AND t.seq=1", [], |row| Ok((row.get(0)?, row.get(1)?))).unwrap();
        assert_eq!(usage, (2., 1.));
        let answer_text_stored: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM keywords WHERE word IN ('initial','result','done','privately')",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(answer_text_stored, 0);
        let dump_text = fs::read_to_string(&dump).unwrap();
        assert!(!dump_text.contains("private widget"));
        assert!(!dump_text.contains("Done privately"));
        assert!(dump_text.contains("nebula.safe-import-dump.v1"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn claude_import_handles_inverted_punctuation_in_prompt_and_answer() {
        let root = fixture();
        let path = root.join("unicode-session.jsonl");
        let lines = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"user","cwd":root,"message":{"content":"¿Puedes revisar esta sesión?"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"assistant","requestId":"r1","message":{"model":"claude-test","content":[{"type":"text","text":"¡Sí! Quedó corregida."}]}}),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();

        let connection = database();
        let result = import_with_roots(
            &connection,
            1,
            &[],
            None,
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();

        assert_eq!(
            result,
            ImportSummary {
                sessions: 1,
                turns: 1,
                skipped: 0
            }
        );
        let turns: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM turns WHERE session_id='unicode-session'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(turns, 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn score_import_validates_scores_and_applies_metadata_to_correct_turn() {
        let root = fixture();
        let connection = database();
        connection
            .execute(
                "INSERT INTO sessions(id,started_at) VALUES('s','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO turns(session_id,seq,started_at) VALUES('s',1,'2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        let first = connection.last_insert_rowid();
        connection
            .execute(
                "INSERT INTO turns(session_id,seq,started_at) VALUES('s',2,'2026-01-01T00:00:01Z')",
                [],
            )
            .unwrap();
        let second = connection.last_insert_rowid();
        let valid = root.join("scores.json");
        fs::write(&valid, json!([{"turn_id":second,"u2a":scores(120.),"a2u":scores(25.),"keywords":["rust"],"task_type":"coding","clarifying_questions":2,"corrects_previous":true},{"turn_id":9999,"u2a":scores(1.)}]).to_string()).unwrap();
        import_scores(&connection, &valid, &english()).unwrap();
        let count: i64 = connection.query_row("SELECT COUNT(*) FROM metric_values v JOIN metric_definitions d ON d.id=v.metric_id WHERE v.turn_id=?1 AND d.parent_id IS NOT NULL AND v.source='model'", [second], |row| row.get(0)).unwrap();
        assert_eq!(count, 20);
        let correction: f64 = connection.query_row("SELECT value FROM metric_values v JOIN metric_definitions d ON d.id=v.metric_id WHERE v.turn_id=?1 AND d.key='corrections'", [first], |row| row.get(0)).unwrap();
        assert_eq!(correction, 1.);
        let invalid = root.join("invalid.json");
        fs::write(
            &invalid,
            json!([{"turn_id":second,"u2a":scores(10.),"a2u":{"ambiguity":1}}]).to_string(),
        )
        .unwrap();
        let before: i64 = connection
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .unwrap();
        assert!(
            import_scores(&connection, &invalid, &english())
                .unwrap_err()
                .to_string()
                .contains("missing")
        );
        let after: i64 = connection
            .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row.get(0))
            .unwrap();
        assert_eq!(
            after, before,
            "invalid score file must roll back atomically"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn codex_shape_imports_session_metadata_and_turn() {
        let root = fixture();
        let path = root.join("rollout.jsonl");
        let lines = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"session_meta","payload":{"id":"codex-session","cwd":root,"git":{"branch":"feature/NEB-8","repository_url":"https://github.com/acme/repo.git"}}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"turn_context","payload":{"model":"gpt-test"}}),
            json!({"timestamp":"2026-01-01T00:00:02Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}),
            json!({"timestamp":"2026-01-01T00:00:03Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"Build src/main.rs"}]}}),
            json!({"timestamp":"2026-01-01T00:00:05Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"Implemented"}]}}),
        ];
        fs::write(
            &path,
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let connection = database();
        let result = import_with_roots(
            &connection,
            1,
            &[],
            None,
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(result.turns, 1);
        let metadata: (String, String, String) = connection
            .query_row(
                "SELECT repo_url,ticket,model FROM sessions WHERE id='codex-session'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            metadata,
            ("acme/repo".into(), "NEB-8".into(), "gpt-test".into())
        );
        fs::remove_dir_all(root).unwrap();
    }
}
