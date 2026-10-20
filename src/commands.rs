use crate::{
    app::App,
    cli::{End, MetricCmd, Start},
    db::{add_keywords, metric_id, open_turn, put_metric},
    i18n::Localizer,
    scoring::parse_scores,
};
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

pub(crate) fn log_start(connection: &Connection, args: Start, localizer: &Localizer) -> Result<()> {
    let (turn, session, sequence, _) = open_turn(connection, args.session.as_deref())
        .map_err(|_| anyhow::anyhow!(localizer.text("log.no_turn")))?;
    put_metric(
        connection,
        turn,
        "estimated_seconds",
        args.estimate,
        "model",
        None,
    )?;
    if let Some(clarifying) = args.clarifying {
        put_metric(
            connection,
            turn,
            "clarifying_questions",
            clarifying as f64,
            "model",
            None,
        )?
    }
    if let Some(correction) = args.correction.filter(|_| sequence > 1) {
        let previous: i64 = connection.query_row(
            "SELECT id FROM turns WHERE session_id=?1 AND seq=?2",
            params![session, sequence - 1],
            |row| row.get(0),
        )?;
        put_metric(
            connection,
            previous,
            "corrections",
            correction as f64,
            "model",
            None,
        )?
    }
    if let Some(kind) = args.kind {
        connection.execute(
            "UPDATE turns SET task_type=?1 WHERE id=?2",
            params![kind, turn],
        )?;
    }
    add_keywords(connection, turn, &args.keywords)?;
    for (key, value) in parse_scores(&args.u2a, "u2a", localizer)? {
        put_metric(connection, turn, &key, value, "model", Some("u2a"))?
    }
    let neb = nebulosity(connection, turn, "u2a")?;
    println!(
        "{}",
        localizer.format(
            "log.start",
            &[("seq", sequence.to_string()), ("neb", display(neb))]
        )
    );
    Ok(())
}

pub(crate) fn log_end(
    app: &App,
    connection: &Connection,
    args: End,
    localizer: &Localizer,
) -> Result<()> {
    let tip = args
        .tip
        .as_deref()
        .map(str::trim)
        .filter(|tip| !tip.is_empty());
    if let Some(tip) = tip {
        let length = tip.chars().count();
        if length > 90 {
            anyhow::bail!(localizer.format(
                "log.tip_long",
                &[("n", length.to_string()), ("max", "90".into())]
            ));
        }
    }
    let (turn, session, sequence, _) = open_turn(connection, args.session.as_deref())
        .map_err(|_| anyhow::anyhow!(localizer.text("log.no_turn")))?;
    for (key, value) in parse_scores(&args.a2u, "a2u", localizer)? {
        put_metric(connection, turn, &key, value, "model", Some("a2u"))?
    }
    if let Some(outcome) = args.outcome {
        connection.execute(
            "UPDATE turns SET outcome=?1 WHERE id=?2",
            params![outcome.as_str(), turn],
        )?;
    }
    let mut state = app.load_state(&session);
    state.insert("model_closed".into(), true.into());
    if let Some(tip) = tip {
        state.insert("tip".into(), tip.into());
    }
    app.save_state(&session, &state)?;
    let neb = nebulosity(connection, turn, "a2u")?;
    println!(
        "{}",
        localizer.format(
            "log.end",
            &[("seq", sequence.to_string()), ("neb", display(neb))]
        )
    );
    Ok(())
}

pub(crate) fn metrics(
    connection: &Connection,
    command: MetricCmd,
    localizer: &Localizer,
) -> Result<()> {
    match command {
        MetricCmd::List => {
            let mut query = connection.prepare(
                "SELECT d.key,d.category,d.unit,d.better,p.key,d.weight,d.directional
                 FROM metric_definitions d
                 LEFT JOIN metric_definitions p ON p.id=d.parent_id
                 ORDER BY d.category,d.id",
            )?;
            for row in query.query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, f64>(5)?,
                    row.get::<_, bool>(6)?,
                ))
            })? {
                let (key, category, unit, better, parent, weight, directional) = row?;
                println!(
                    "{category:<12} {key:<24} {unit:<12} {better:<6} parent={} w={weight} dir={}",
                    parent.as_deref().unwrap_or("-"),
                    i32::from(directional)
                )
            }
        }
        MetricCmd::Add {
            key,
            name,
            category,
            unit,
            better,
            parent,
            weight,
            aggregation,
            directional,
        } => {
            let parent = parent
                .map(|key| {
                    metric_id(connection, &key).map_err(|_| {
                        anyhow::anyhow!(localizer.format("error.unknown_metric", &[("key", key)]))
                    })
                })
                .transpose()?;
            connection.execute("INSERT INTO metric_definitions(key,name,category,unit,better,parent_id,weight,aggregation,directional)VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9)",params![key,name,category,unit,better.as_str(),parent,weight,aggregation,directional])?;
            println!(
                "{}",
                localizer.format("admin.metric_added", &[("key", key)])
            )
        }
    }
    Ok(())
}

fn nebulosity(connection: &Connection, turn: i64, direction: &str) -> Result<Option<f64>> {
    Ok(connection
        .query_row(
            "SELECT nebulosity FROM v_nebulosity WHERE turn_id=?1 AND direction=?2 AND source='model'",
            params![turn, direction],
            |row| row.get(0),
        )
        .optional()?)
}

fn display(value: Option<f64>) -> String {
    value.map_or_else(|| "?".into(), |value| format!("{value:.1}"))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{db, scoring::KEYS};
    use serde_json::{Value, json};
    use std::fs;

    fn english() -> Localizer {
        Localizer::load(crate::i18n::Lang::En).unwrap()
    }

    fn memory_db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        db::initialize(&connection).unwrap();
        connection
    }

    fn test_app(name: &str) -> App {
        App::for_test(
            std::env::temp_dir().join(format!("nebula-test-{}-{name}", std::process::id())),
        )
    }

    fn add_turn(connection: &Connection, session: &str, seq: i64, started_at: &str) -> i64 {
        connection
            .execute(
                "INSERT OR IGNORE INTO sessions(id,started_at)VALUES(?1,?2)",
                params![session, started_at],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO turns(session_id,seq,started_at)VALUES(?1,?2,?3)",
                params![session, seq, started_at],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    fn scores(value: f64) -> String {
        Value::Object(
            KEYS.into_iter()
                .map(|key| (key.to_owned(), json!(value)))
                .collect(),
        )
        .to_string()
    }

    #[test]
    fn log_start_records_metadata_scores_keywords_and_correction() {
        let connection = memory_db();
        let previous = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        connection
            .execute(
                "UPDATE turns SET ended_at=started_at WHERE id=?1",
                [previous],
            )
            .unwrap();
        let current = add_turn(&connection, "s1", 2, "2026-01-02T00:00:00.000Z");

        log_start(
            &connection,
            Start {
                estimate: 12.5,
                keywords: "Rust, Tests".into(),
                kind: Some("coding".into()),
                clarifying: Some(2),
                correction: Some(1),
                u2a: scores(25.0),
                session: Some("s1".into()),
                lang: None,
            },
            &english(),
        )
        .unwrap();

        assert_eq!(
            db::get_metric(&connection, current, "estimated_seconds").unwrap(),
            Some(12.5)
        );
        assert_eq!(
            db::get_metric(&connection, current, "clarifying_questions").unwrap(),
            Some(2.0)
        );
        assert_eq!(
            db::get_metric(&connection, previous, "corrections").unwrap(),
            Some(1.0)
        );
        assert_eq!(
            db::get_metric(&connection, current, "ambiguity").unwrap(),
            Some(25.0)
        );
        let kind: String = connection
            .query_row(
                "SELECT task_type FROM turns WHERE id=?1",
                [current],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(kind, "coding");
    }

    #[test]
    fn log_start_accepts_optional_fields_and_first_turn_correction() {
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        log_start(
            &connection,
            Start {
                estimate: 1.0,
                keywords: String::new(),
                kind: None,
                clarifying: None,
                correction: Some(1),
                u2a: scores(0.0),
                session: None,
                lang: None,
            },
            &english(),
        )
        .unwrap();
        assert_eq!(
            db::get_metric(&connection, turn, "corrections").unwrap(),
            None
        );
    }

    #[test]
    fn log_end_rejects_overlong_unicode_tip_without_mutating_turn() {
        let app = test_app("log-end");
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        let error = log_end(
            &app,
            &connection,
            End {
                a2u: scores(75.0),
                outcome: Some(crate::cli::Outcome::Partial),
                tip: Some("🦀".repeat(91)),
                session: Some("s1".into()),
                lang: None,
            },
            &english(),
        )
        .unwrap_err();

        assert!(error.to_string().contains("91 characters (max 90)"));
        assert_eq!(db::get_metric(&connection, turn, "opacity").unwrap(), None);
        let outcome: Option<String> = connection
            .query_row("SELECT outcome FROM turns WHERE id=?1", [turn], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(outcome, None);
        let state = app.load_state("s1");
        assert!(state.get("model_closed").is_none());
    }

    #[test]
    fn log_end_stores_trimmed_tip_at_unicode_limit() {
        let app = test_app("log-end-tip-limit");
        let connection = memory_db();
        add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        let tip = format!("  {}  ", "é".repeat(90));
        log_end(
            &app,
            &connection,
            End {
                a2u: scores(0.0),
                outcome: None,
                tip: Some(tip),
                session: Some("s1".into()),
                lang: None,
            },
            &english(),
        )
        .unwrap();
        assert_eq!(
            app.load_state("s1")["tip"]
                .as_str()
                .unwrap()
                .chars()
                .count(),
            90
        );
        fs::remove_dir_all(app.state_path("s1").parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn log_end_accepts_missing_optional_fields() {
        let app = test_app("log-end-optional");
        let connection = memory_db();
        add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        log_end(
            &app,
            &connection,
            End {
                a2u: scores(0.0),
                outcome: None,
                tip: None,
                session: None,
                lang: None,
            },
            &english(),
        )
        .unwrap();
        assert!(app.load_state("s1")["model_closed"].as_bool().unwrap());
        fs::remove_dir_all(app.state_path("s1").parent().unwrap().parent().unwrap()).unwrap();
    }

    #[test]
    fn metrics_list_and_add_support_parent_and_validation() {
        let connection = memory_db();
        metrics(&connection, MetricCmd::List, &english()).unwrap();
        metrics(
            &connection,
            MetricCmd::Add {
                key: "custom".into(),
                name: "Custom".into(),
                category: "quality".into(),
                unit: "count".into(),
                better: crate::cli::Better::Higher,
                parent: Some("nebulosity".into()),
                weight: 2.5,
                aggregation: "avg".into(),
                directional: true,
            },
            &english(),
        )
        .unwrap();
        let row: (f64, String, i64) = connection
            .query_row(
                "SELECT weight,aggregation,directional FROM metric_definitions WHERE key='custom'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, (2.5, "avg".into(), 1));
        assert!(
            metrics(
                &connection,
                MetricCmd::Add {
                    key: "bad".into(),
                    name: "Bad".into(),
                    category: "quality".into(),
                    unit: "count".into(),
                    better: crate::cli::Better::Lower,
                    parent: Some("missing".into()),
                    weight: 1.0,
                    aggregation: "sum".into(),
                    directional: false,
                },
                &english(),
            )
            .is_err()
        );
    }
}
