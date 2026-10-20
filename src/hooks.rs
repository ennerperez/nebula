use crate::{
    app::App,
    cli::Event,
    db::{add_keywords, get_metric, open_turn, put_metric, timestamp, u2a_drop},
    i18n::Localizer,
    learning::learned_heuristic,
    scoring::{
        asks_question, fallback_keywords, looks_like_correction, unmarked_questions, word_count,
    },
};
use anyhow::Result;
use chrono::{DateTime, FixedOffset};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Map, Value, json};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

static TICKET: Lazy<Regex> = Lazy::new(|| Regex::new(r"[A-Z][A-Z0-9]+-\d+").unwrap());

#[derive(Default, Debug, PartialEq)]
struct TranscriptMetrics {
    server_requests: usize,
    agents: usize,
    words_out: usize,
    model: Option<String>,
    last_text: String,
}

pub(crate) fn handle(
    app: &App,
    connection: &Connection,
    event: Event,
    payload: Value,
    localizer: &Localizer,
) -> Result<()> {
    let session = payload
        .get("session_id")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    match event {
        Event::UserPromptSubmit => user_prompt(app, connection, session, &payload, localizer),
        Event::Notification => notification(app, session, &payload),
        Event::Stop => stop(app, connection, session, &payload, localizer),
    }
}

fn user_prompt(
    app: &App,
    connection: &Connection,
    session: &str,
    payload: &Value,
    localizer: &Localizer,
) -> Result<()> {
    let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or("");
    if prompt.contains("<agent-message") || prompt.contains("<task-notification") {
        return Ok(());
    }
    let started_at = timestamp();
    let previous_state = app.load_state(session);
    let last_stop = previous_state.get("last_stop").and_then(Value::as_str);
    connection.execute(
        "UPDATE turns SET ended_at=?1,outcome=IFNULL(outcome,?2) WHERE session_id=?3 AND ended_at IS NULL",
        params![last_stop.unwrap_or(&started_at), if last_stop.is_some() { "completed" } else { "abandoned" }, session],
    )?;

    let exists = connection
        .query_row("SELECT 1 FROM sessions WHERE id=?1", [session], |_| Ok(()))
        .optional()?
        .is_some();
    if !exists {
        let cwd = payload
            .get("cwd")
            .and_then(Value::as_str)
            .map(PathBuf::from)
            .unwrap_or(std::env::current_dir()?);
        let branch = git(&cwd, &["rev-parse", "--abbrev-ref", "HEAD"]);
        let remote = git(&cwd, &["remote", "get-url", "origin"]);
        let ticket = branch
            .as_deref()
            .and_then(|branch| TICKET.find(branch))
            .map(|found| found.as_str());
        connection.execute(
            "INSERT INTO sessions(id,started_at,repo_url,branch,ticket)VALUES(?1,?2,?3,?4,?5)",
            params![
                session,
                started_at,
                remote.as_deref().and_then(repo_path),
                branch,
                ticket
            ],
        )?;
    }
    let sequence: i64 = connection.query_row(
        "SELECT IFNULL(MAX(seq),0)+1 FROM turns WHERE session_id=?1",
        [session],
        |row| row.get(0),
    )?;
    connection.execute(
        "INSERT INTO turns(session_id,seq,started_at)VALUES(?1,?2,?3)",
        params![session, sequence, started_at],
    )?;
    let turn = connection.last_insert_rowid();
    put_metric(
        connection,
        turn,
        "words_in",
        word_count(prompt) as f64,
        "hook",
        None,
    )?;
    let transcript = payload
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(Path::new);
    let replies = transcript
        .map(last_assistant_text)
        .transpose()?
        .is_some_and(|text| asks_question(&text));
    for (key, value) in learned_heuristic(connection, prompt, replies, "u2a")? {
        put_metric(connection, turn, &key, value, "heuristic", Some("u2a"))?;
    }
    put_metric(
        connection,
        turn,
        "unmarked_questions",
        unmarked_questions(prompt) as f64,
        "heuristic",
        Some("u2a"),
    )?;
    if sequence > 1 && looks_like_correction(prompt, replies) {
        if let Some(previous) = connection
            .query_row(
                "SELECT id FROM turns WHERE session_id=?1 AND seq=?2",
                params![session, sequence - 1],
                |row| row.get::<_, i64>(0),
            )
            .optional()?
        {
            put_metric(connection, previous, "corrections", 1., "heuristic", None)?;
        }
    }
    let mut state = Map::new();
    state.insert("turn_id".into(), turn.into());
    state.insert("waits".into(), json!([]));
    state.insert(
        "fallback_keywords".into(),
        json!(fallback_keywords(prompt, 5)),
    );
    state.insert("prompt_question".into(), asks_question(prompt).into());
    app.save_state(session, &state)?;
    let context = localizer.format("hook.context", &[("seq", sequence.to_string())]);
    println!(
        "{}",
        json!({"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":context}})
    );
    Ok(())
}

fn notification(app: &App, session: &str, payload: &Value) -> Result<()> {
    let kind = format!(
        "{} {}",
        payload
            .get("notification_type")
            .and_then(Value::as_str)
            .unwrap_or(""),
        payload.get("message").and_then(Value::as_str).unwrap_or("")
    );
    if !kind.to_lowercase().contains("permission") {
        return Ok(());
    }
    let mut state = app.load_state(session);
    if state.is_empty() {
        return Ok(());
    }
    let waits = state.entry("waits").or_insert_with(|| json!([]));
    if !waits.is_array() {
        *waits = json!([]);
    }
    if let Some(waits) = waits.as_array_mut() {
        waits.push(timestamp().into());
    }
    app.save_state(session, &state)
}

fn stop(
    app: &App,
    connection: &Connection,
    session: &str,
    payload: &Value,
    localizer: &Localizer,
) -> Result<()> {
    let (turn, _, _, started_at) = match open_turn(connection, Some(session)) {
        Ok(turn) => turn,
        Err(_) => return Ok(()),
    };
    let mut state = app.load_state(session);
    let ended_at = timestamp();
    let start = parse_time(&started_at)?;
    let seconds = (parse_time(&ended_at)? - start).num_milliseconds() as f64 / 1000.;
    put_metric(
        connection,
        turn,
        "real_seconds",
        round1(seconds),
        "hook",
        None,
    )?;

    let transcript_path = payload
        .get("transcript_path")
        .and_then(Value::as_str)
        .map(Path::new);
    let metrics = transcript_path
        .map(|path| transcript_metrics(path, start, None))
        .transpose()?
        .unwrap_or_default();
    put_metric(
        connection,
        turn,
        "server_requests",
        metrics.server_requests as f64,
        "transcript",
        None,
    )?;
    put_metric(
        connection,
        turn,
        "agents",
        metrics.agents as f64,
        "transcript",
        None,
    )?;
    if let Some(model) = metrics.model.as_deref() {
        connection.execute(
            "UPDATE sessions SET model=?1 WHERE id=?2",
            params![model, session],
        )?;
    }
    let payload_last = payload
        .get("last_assistant_message")
        .and_then(Value::as_str)
        .unwrap_or("");
    let last = if payload_last.is_empty() {
        metrics.last_text.as_str()
    } else {
        payload_last
    };
    let missing_final = !payload_last.is_empty()
        && (metrics.last_text.is_empty() || !payload_last.contains(&metrics.last_text));
    let words_out = metrics.words_out
        + if missing_final || transcript_path.is_none() {
            word_count(last)
        } else {
            0
        };
    put_metric(
        connection,
        turn,
        "words_out",
        words_out as f64,
        "transcript",
        None,
    )?;
    let replies = state
        .get("prompt_question")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    for (key, value) in learned_heuristic(connection, last, replies, "a2u")? {
        put_metric(connection, turn, &key, value, "heuristic", Some("a2u"))?;
    }
    put_metric(
        connection,
        turn,
        "unmarked_questions",
        unmarked_questions(last) as f64,
        "heuristic",
        Some("a2u"),
    )?;
    let waits = state
        .get("waits")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let blocked = transcript_path
        .map(|path| blocked_seconds(path, waits))
        .transpose()?
        .unwrap_or(0.);
    put_metric(connection, turn, "blocked_seconds", blocked, "hook", None)?;
    if let Some(estimate) =
        get_metric(connection, turn, "estimated_seconds")?.filter(|estimate| *estimate != 0.)
    {
        put_metric(
            connection,
            turn,
            "estimate_ratio",
            round2(seconds / estimate),
            "hook",
            None,
        )?;
    }
    let has_keywords = connection
        .query_row(
            "SELECT 1 FROM turn_keywords WHERE turn_id=?1",
            [turn],
            |_| Ok(()),
        )
        .optional()?
        .is_some();
    if !has_keywords {
        let keywords = state
            .get("fallback_keywords")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect::<Vec<_>>()
            .join(",");
        add_keywords(connection, turn, &keywords)?;
    }
    connection.execute(
        "UPDATE sessions SET ended_at=?1 WHERE id=?2",
        params![ended_at, session],
    )?;
    if !state
        .get("model_closed")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        state.insert("last_stop".into(), ended_at.into());
        app.save_state(session, &state)?;
        return Ok(());
    }
    connection.execute(
        "UPDATE turns SET ended_at=?1,outcome=IFNULL(outcome,'completed') WHERE id=?2",
        params![ended_at, turn],
    )?;
    let explicit_tip = state.get("tip").and_then(Value::as_str);
    let tip = turn_tip(connection, turn, explicit_tip)?;
    let _ = fs::remove_file(app.state_path(session));
    println!(
        "{}",
        json!({"systemMessage":summary(connection, turn, tip.as_deref(), localizer)?})
    );
    Ok(())
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
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn repo_path(url: &str) -> Option<&str> {
    let trimmed = url.trim_end_matches(".git").trim_end_matches('/');
    let separator = trimmed.rfind([':', '/'])?;
    let owner_start = trimmed[..separator]
        .rfind([':', '/'])
        .map_or(0, |index| index + 1);
    Some(&trimmed[owner_start..])
}

fn parse_time(value: &str) -> Result<DateTime<FixedOffset>> {
    Ok(DateTime::parse_from_rfc3339(value)?)
}
fn round1(value: f64) -> f64 {
    (value * 10.).round() / 10.
}
fn round2(value: f64) -> f64 {
    (value * 100.).round() / 100.
}

fn jsonl(path: &Path) -> Vec<Value> {
    fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn last_assistant_text(path: &Path) -> Result<String> {
    let mut last = String::new();
    for entry in jsonl(path) {
        if entry.get("type").and_then(Value::as_str) != Some("assistant")
            || entry
                .get("isSidechain")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            continue;
        }
        for block in entry
            .pointer("/message/content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            if block.get("type").and_then(Value::as_str) == Some("text")
                && let Some(text) = block
                    .get("text")
                    .and_then(Value::as_str)
                    .filter(|text| !text.trim().is_empty())
            {
                last = text.to_owned();
            }
        }
    }
    Ok(last)
}

fn transcript_metrics(
    path: &Path,
    start: DateTime<FixedOffset>,
    end: Option<DateTime<FixedOffset>>,
) -> Result<TranscriptMetrics> {
    let mut files = vec![path.to_owned()];
    let mut subagents = path.with_extension("");
    subagents.push("subagents");
    if let Ok(entries) = fs::read_dir(subagents) {
        let mut paths = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
            .collect::<Vec<_>>();
        paths.sort();
        files.extend(paths);
    }
    let mut result = TranscriptMetrics::default();
    let mut requests = BTreeSet::new();
    for (index, file) in files.iter().enumerate() {
        let main = index == 0;
        for entry in jsonl(file) {
            if entry.get("type").and_then(Value::as_str) != Some("assistant") {
                continue;
            }
            let Some(time) = entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|value| parse_time(value).ok())
            else {
                continue;
            };
            if time < start || end.is_some_and(|end| time >= end) {
                continue;
            }
            let message = entry.get("message").unwrap_or(&Value::Null);
            if let Some(id) = entry
                .get("requestId")
                .and_then(Value::as_str)
                .or_else(|| message.get("id").and_then(Value::as_str))
            {
                requests.insert(id.to_owned());
            }
            if main && let Some(model) = message.get("model").and_then(Value::as_str) {
                result.model = Some(model.to_owned());
            }
            for block in message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                match block.get("type").and_then(Value::as_str) {
                    Some("tool_use")
                        if matches!(
                            block.get("name").and_then(Value::as_str),
                            Some("Agent" | "Task")
                        ) =>
                    {
                        result.agents += 1
                    }
                    Some("text") if main => {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            result.words_out += word_count(text);
                            if !text.trim().is_empty() {
                                result.last_text = text.to_owned();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    result.server_requests = requests.len();
    Ok(result)
}

fn blocked_seconds(path: &Path, waits: &[Value]) -> Result<f64> {
    let mut stamps = jsonl(path)
        .into_iter()
        .filter_map(|entry| {
            entry
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|value| parse_time(value).ok())
        })
        .collect::<Vec<_>>();
    stamps.sort();
    let total = waits
        .iter()
        .filter_map(Value::as_str)
        .filter_map(|wait| parse_time(wait).ok())
        .filter_map(|wait| {
            stamps
                .iter()
                .find(|stamp| **stamp > wait)
                .map(|next| (*next - wait).num_milliseconds() as f64 / 1000.)
        })
        .sum::<f64>();
    Ok(round1(total))
}

fn nebulosity(
    connection: &Connection,
    turn: i64,
    direction: &str,
    source: &str,
) -> Result<Option<f64>> {
    Ok(connection
        .query_row(
            "SELECT nebulosity FROM v_nebulosity WHERE turn_id=?1 AND direction=?2 AND source=?3",
            params![turn, direction, source],
            |row| row.get(0),
        )
        .optional()?)
}

fn turn_tip(connection: &Connection, turn: i64, explicit: Option<&str>) -> Result<Option<String>> {
    let Some(tip) = explicit.filter(|tip| !tip.is_empty()) else {
        return Ok(None);
    };
    let (now, baseline) = u2a_drop(connection, turn)?;
    Ok(match (now, baseline) {
        (Some(now), Some(baseline)) if now > baseline => Some(tip.to_owned()),
        _ => None,
    })
}

fn seconds(value: Option<f64>) -> String {
    value.map_or_else(
        || "?".into(),
        |value| {
            let value = value as i64;
            if value >= 60 {
                format!("{}m{:02}s", value / 60, value % 60)
            } else {
                format!("{value}s")
            }
        },
    )
}
fn compact(value: Option<f64>) -> String {
    value.map_or_else(
        || "?".into(),
        |value| {
            if value >= 1000. {
                format!("{:.1}k", value / 1000.)
            } else {
                format!("{}", value as i64)
            }
        },
    )
}
fn neb(value: Option<f64>) -> String {
    value.map_or_else(|| "?".into(), |value| format!("{value}"))
}

fn summary(
    connection: &Connection,
    turn: i64,
    tip: Option<&str>,
    localizer: &Localizer,
) -> Result<String> {
    let estimate = get_metric(connection, turn, "estimated_seconds")?;
    let real = get_metric(connection, turn, "real_seconds")?;
    let ratio = get_metric(connection, turn, "estimate_ratio")?;
    let profile = |direction: &str| -> Result<(String, String)> {
        Ok((
            neb(nebulosity(connection, turn, direction, "model")?),
            neb(nebulosity(connection, turn, direction, "heuristic")?),
        ))
    };
    let line1 = localizer.format(
        "summary.line1",
        &[
            ("est", seconds(estimate)),
            ("real", seconds(real)),
            (
                "ratio",
                ratio.map_or_else(String::new, |value| format!(" ({value}x)")),
            ),
            (
                "req",
                compact(get_metric(connection, turn, "server_requests")?),
            ),
            ("win", compact(get_metric(connection, turn, "words_in")?)),
            ("wout", compact(get_metric(connection, turn, "words_out")?)),
            ("agents", compact(get_metric(connection, turn, "agents")?)),
        ],
    );
    let suffix = tip.map_or_else(String::new, |tip| {
        localizer.format("summary.tip", &[("tip", tip.to_owned())])
    });
    let (u2a, u2a_h) = profile("u2a")?;
    let (a2u, a2u_h) = profile("a2u")?;
    let line2 = localizer.format(
        "summary.line2",
        &[
            ("u2a", u2a),
            ("u2a_h", u2a_h),
            ("a2u", a2u),
            ("a2u_h", a2u_h),
            (
                "clar",
                compact(Some(
                    get_metric(connection, turn, "clarifying_questions")?.unwrap_or(0.),
                )),
            ),
            (
                "corr",
                compact(Some(
                    get_metric(connection, turn, "corrections")?.unwrap_or(0.),
                )),
            ),
            ("tip", suffix),
        ],
    );
    Ok(format!("{line1}\n{line2}"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::db;
    use crate::scoring::KEYS;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    fn english() -> Localizer {
        Localizer::load(crate::i18n::Lang::En).unwrap()
    }
    fn spanish() -> Localizer {
        Localizer::load(crate::i18n::Lang::Es).unwrap()
    }
    fn setup(name: &str) -> (App, Connection, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "nebula-hooks-{}-{name}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let app = App::for_test(root.join("home"));
        let connection = Connection::open_in_memory().unwrap();
        db::initialize(&connection).unwrap();
        (app, connection, root)
    }
    fn write_jsonl(path: &Path, entries: &[Value]) {
        fs::write(
            path,
            entries
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
    }

    fn scored_turn(connection: &Connection, sequence: i64, value: f64) -> i64 {
        connection
            .execute(
                "INSERT OR IGNORE INTO sessions(id,started_at) VALUES('tips','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO turns(session_id,seq,started_at) VALUES('tips',?1,?2)",
                params![sequence, format!("2026-01-{sequence:02}T00:00:00Z")],
            )
            .unwrap();
        let turn = connection.last_insert_rowid();
        for key in KEYS {
            db::put_metric(connection, turn, key, value, "model", Some("u2a")).unwrap();
        }
        turn
    }

    #[test]
    fn explicit_tip_is_gated_by_recent_same_session_baseline() {
        let (_, connection, root) = setup("tip-gate");
        let first = scored_turn(&connection, 1, 30.0);
        let improved = scored_turn(&connection, 2, 20.0);
        let worse = scored_turn(&connection, 3, 50.0);

        assert_eq!(
            turn_tip(&connection, first, Some("agent tip")).unwrap(),
            None
        );
        assert_eq!(
            turn_tip(&connection, improved, Some("agent tip")).unwrap(),
            None
        );
        assert_eq!(
            turn_tip(&connection, worse, Some("agent tip")).unwrap(),
            Some("agent tip".into())
        );
        assert_eq!(turn_tip(&connection, worse, None).unwrap(), None);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn summary_uses_readable_labels_and_direction_suffixes() {
        let (_, connection, root) = setup("summary-format");
        let turn = scored_turn(&connection, 1, 3.4);
        for key in KEYS {
            put_metric(&connection, turn, key, 12.0, "heuristic", Some("u2a")).unwrap();
            put_metric(&connection, turn, key, 1.0, "model", Some("a2u")).unwrap();
            put_metric(&connection, turn, key, 8.0, "heuristic", Some("a2u")).unwrap();
        }
        for (key, value) in [
            ("estimated_seconds", 45.0),
            ("real_seconds", 38.0),
            ("estimate_ratio", 0.84),
            ("server_requests", 3.0),
            ("words_in", 8.0),
            ("words_out", 96.0),
            ("agents", 0.0),
            ("clarifying_questions", 0.0),
            ("corrections", 0.0),
        ] {
            put_metric(&connection, turn, key, value, "hook", None).unwrap();
        }

        assert_eq!(
            summary(
                &connection,
                turn,
                Some("Indica archivo y resultado esperado."),
                &spanish(),
            )
            .unwrap(),
            "⏱ est 45s / real 38s (0.84x) · pet.: 3 · palabras: 8u / 96a · agentes: 0\n\
🌫 u2a 3.4 / (12 heur.) · a2u 1 / (8 heur.) · aclaraciones: 0 · correcciones: 0 · consejo: Indica archivo y resultado esperado."
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn prompt_ignores_subagent_handback_and_recovers_previous_stop() {
        let (app, connection, root) = setup("recovery");
        handle(
            &app,
            &connection,
            Event::UserPromptSubmit,
            json!({"session_id":"s","prompt":"first request"}),
            &english(),
        )
        .unwrap();
        let first = open_turn(&connection, Some("s")).unwrap().0;
        let mut state = app.load_state("s");
        state.insert("last_stop".into(), "2026-01-02T00:00:00.000Z".into());
        app.save_state("s", &state).unwrap();
        handle(
            &app,
            &connection,
            Event::UserPromptSubmit,
            json!({"session_id":"s","prompt":"<agent-message>done"}),
            &english(),
        )
        .unwrap();
        assert_eq!(open_turn(&connection, Some("s")).unwrap().0, first);
        handle(
            &app,
            &connection,
            Event::UserPromptSubmit,
            json!({"session_id":"s","prompt":"second request"}),
            &english(),
        )
        .unwrap();
        let second = open_turn(&connection, Some("s")).unwrap().0;
        handle(
            &app,
            &connection,
            Event::UserPromptSubmit,
            json!({"session_id":"s","prompt":"No, wrong result"}),
            &english(),
        )
        .unwrap();
        assert_eq!(
            get_metric(&connection, second, "corrections").unwrap(),
            Some(1.)
        );
        let closed: (String, String) = connection
            .query_row(
                "SELECT ended_at,outcome FROM turns WHERE id=?1",
                [first],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            closed,
            ("2026-01-02T00:00:00.000Z".into(), "completed".into())
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn transcript_aggregates_main_and_subagents_and_flushes_final_text() {
        let (app, connection, root) = setup("transcript");
        let transcript = root.join("session.jsonl");
        let sub = root.join("session/subagents");
        fs::create_dir_all(&sub).unwrap();
        write_jsonl(
            &transcript,
            &[
                json!({"type":"assistant","timestamp":"2099-01-01T00:00:01Z","requestId":"r1","message":{"model":"claude-x","content":[{"type":"tool_use","name":"Task"},{"type":"text","text":"draft answer"}]}}),
            ],
        );
        write_jsonl(
            &sub.join("a.jsonl"),
            &[
                json!({"type":"assistant","timestamp":"2099-01-01T00:00:02Z","requestId":"r2","message":{"content":[{"type":"tool_use","name":"Agent"},{"type":"text","text":"worker words ignored"}]}}),
            ],
        );
        let tm = transcript_metrics(
            &transcript,
            parse_time("2099-01-01T00:00:00Z").unwrap(),
            None,
        )
        .unwrap();
        assert_eq!(
            (
                tm.server_requests,
                tm.agents,
                tm.words_out,
                tm.model.as_deref()
            ),
            (2, 2, 2, Some("claude-x"))
        );
        handle(
            &app,
            &connection,
            Event::UserPromptSubmit,
            json!({"session_id":"s","prompt":"request","transcript_path":transcript}),
            &english(),
        )
        .unwrap();
        let turn = open_turn(&connection, Some("s")).unwrap().0;
        let mut state = app.load_state("s");
        state.insert("model_closed".into(), true.into());
        app.save_state("s", &state).unwrap();
        handle(&app, &connection, Event::Stop, json!({"session_id":"s","transcript_path":transcript,"last_assistant_message":"final answer now"}), &english()).unwrap();
        assert_eq!(
            get_metric(&connection, turn, "words_out").unwrap(),
            Some(5.)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn notification_waits_correction_keywords_and_intermediate_stop_are_recorded() {
        let (app, connection, root) = setup("lifecycle");
        let transcript = root.join("turn.jsonl");
        write_jsonl(
            &transcript,
            &[
                json!({"type":"assistant","timestamp":"2099-01-01T00:00:02Z","message":{"content":[{"type":"text","text":"Would you like this?"}]}}),
            ],
        );
        handle(&app, &connection, Event::UserPromptSubmit, json!({"session_id":"s","prompt":"Rust deployment command","transcript_path":transcript}), &english()).unwrap();
        let turn = open_turn(&connection, Some("s")).unwrap().0;
        let mut malformed_state = app.load_state("s");
        malformed_state.insert("waits".into(), "invalid".into());
        app.save_state("s", &malformed_state).unwrap();
        handle(
            &app,
            &connection,
            Event::Notification,
            json!({"session_id":"s","notification_type":"permission_prompt"}),
            &english(),
        )
        .unwrap();
        assert_eq!(
            app.load_state("s")["waits"].as_array().map(Vec::len),
            Some(1)
        );
        handle(&app, &connection, Event::Stop, json!({"session_id":"s","transcript_path":transcript,"last_assistant_message":"intermediate"}), &english()).unwrap();
        assert_eq!(open_turn(&connection, Some("s")).unwrap().0, turn);
        assert!(app.load_state("s").get("last_stop").is_some());
        assert!(
            get_metric(&connection, turn, "blocked_seconds")
                .unwrap()
                .unwrap()
                > 0.
        );
        let keyword_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM turn_keywords WHERE turn_id=?1",
                [turn],
                |r| r.get(0),
            )
            .unwrap();
        assert!(keyword_count > 0);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn repo_paths_are_normalized() {
        assert_eq!(
            repo_path("git@github.com:group/project.git"),
            Some("group/project")
        );
        assert_eq!(
            repo_path("https://github.com/group/project.git"),
            Some("group/project")
        );
    }
}
