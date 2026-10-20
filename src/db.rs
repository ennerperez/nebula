use crate::scoring::KEYS;
use anyhow::{Context, Result};
use chrono::{SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter};

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS sessions(id TEXT PRIMARY KEY,started_at TEXT NOT NULL,ended_at TEXT,repo_url TEXT,branch TEXT,ticket TEXT,model TEXT);
CREATE TABLE IF NOT EXISTS turns(id INTEGER PRIMARY KEY AUTOINCREMENT,session_id TEXT NOT NULL REFERENCES sessions(id),seq INTEGER NOT NULL,started_at TEXT NOT NULL,ended_at TEXT,task_type TEXT,outcome TEXT,UNIQUE(session_id,seq));
CREATE TABLE IF NOT EXISTS keywords(id INTEGER PRIMARY KEY AUTOINCREMENT,word TEXT NOT NULL UNIQUE);
CREATE TABLE IF NOT EXISTS turn_keywords(turn_id INTEGER NOT NULL REFERENCES turns(id),keyword_id INTEGER NOT NULL REFERENCES keywords(id),PRIMARY KEY(turn_id,keyword_id));
CREATE TABLE IF NOT EXISTS metric_definitions(id INTEGER PRIMARY KEY AUTOINCREMENT,key TEXT NOT NULL UNIQUE,name TEXT NOT NULL,category TEXT NOT NULL,unit TEXT NOT NULL,better TEXT NOT NULL CHECK(better IN('lower','higher')),parent_id INTEGER REFERENCES metric_definitions(id),weight REAL NOT NULL DEFAULT 1,aggregation TEXT NOT NULL DEFAULT 'sum',directional INTEGER NOT NULL DEFAULT 0,description TEXT,active INTEGER NOT NULL DEFAULT 1);
CREATE TABLE IF NOT EXISTS metric_values(id INTEGER PRIMARY KEY AUTOINCREMENT,turn_id INTEGER NOT NULL REFERENCES turns(id),metric_id INTEGER NOT NULL REFERENCES metric_definitions(id),direction TEXT CHECK(direction IN('u2a','a2u') OR direction IS NULL),source TEXT NOT NULL CHECK(source IN('model','heuristic','hook','transcript','git')),value REAL NOT NULL,recorded_at TEXT NOT NULL);
CREATE UNIQUE INDEX IF NOT EXISTS ux_metric_values ON metric_values(turn_id,metric_id,IFNULL(direction,''),source);
DROP TABLE IF EXISTS models_pricing;
CREATE TABLE IF NOT EXISTS heuristic_weights(metric_id INTEGER NOT NULL REFERENCES metric_definitions(id),direction TEXT NOT NULL CHECK(direction IN('u2a','a2u')),feature TEXT NOT NULL,weight REAL NOT NULL,PRIMARY KEY(metric_id,direction,feature));
CREATE TABLE IF NOT EXISTS heuristic_calibration(metric_id INTEGER NOT NULL REFERENCES metric_definitions(id),direction TEXT NOT NULL CHECK(direction IN('u2a','a2u')),slope REAL NOT NULL,intercept REAL NOT NULL,correlation REAL,samples INTEGER NOT NULL,fitted_at TEXT NOT NULL,PRIMARY KEY(metric_id,direction));
CREATE TRIGGER IF NOT EXISTS validate_metric_definitions_insert BEFORE INSERT ON metric_definitions
WHEN NEW.better NOT IN('lower','higher') BEGIN SELECT RAISE(ABORT,'invalid better'); END;
CREATE TRIGGER IF NOT EXISTS validate_metric_definitions_update BEFORE UPDATE OF better ON metric_definitions
WHEN NEW.better NOT IN('lower','higher') BEGIN SELECT RAISE(ABORT,'invalid better'); END;
CREATE TRIGGER IF NOT EXISTS validate_metric_values_insert BEFORE INSERT ON metric_values
WHEN (NEW.direction IS NOT NULL AND NEW.direction NOT IN('u2a','a2u'))
  OR NEW.source NOT IN('model','heuristic','hook','transcript','git')
BEGIN SELECT RAISE(ABORT,'invalid metric value protocol'); END;
CREATE TRIGGER IF NOT EXISTS validate_metric_values_update BEFORE UPDATE OF direction,source ON metric_values
WHEN (NEW.direction IS NOT NULL AND NEW.direction NOT IN('u2a','a2u'))
  OR NEW.source NOT IN('model','heuristic','hook','transcript','git')
BEGIN SELECT RAISE(ABORT,'invalid metric value protocol'); END;
CREATE TRIGGER IF NOT EXISTS validate_heuristic_weights_insert BEFORE INSERT ON heuristic_weights
WHEN NEW.direction NOT IN('u2a','a2u') BEGIN SELECT RAISE(ABORT,'invalid direction'); END;
CREATE TRIGGER IF NOT EXISTS validate_heuristic_weights_update BEFORE UPDATE OF direction ON heuristic_weights
WHEN NEW.direction NOT IN('u2a','a2u') BEGIN SELECT RAISE(ABORT,'invalid direction'); END;
CREATE TRIGGER IF NOT EXISTS validate_heuristic_calibration_insert BEFORE INSERT ON heuristic_calibration
WHEN NEW.direction NOT IN('u2a','a2u') BEGIN SELECT RAISE(ABORT,'invalid direction'); END;
CREATE TRIGGER IF NOT EXISTS validate_heuristic_calibration_update BEFORE UPDATE OF direction ON heuristic_calibration
WHEN NEW.direction NOT IN('u2a','a2u') BEGIN SELECT RAISE(ABORT,'invalid direction'); END;
DROP VIEW IF EXISTS v_metric_values;
CREATE VIEW v_metric_values AS
SELECT v.id,v.turn_id,v.metric_id,v.direction,v.source,v.recorded_at,v.value raw_value,
CASE WHEN v.source='heuristic' AND h.slope IS NOT NULL
THEN ROUND(MIN(100.0,MAX(0.0,v.value*h.slope+h.intercept)),1)
ELSE v.value END value
FROM metric_values v LEFT JOIN heuristic_calibration h
ON h.metric_id=v.metric_id AND h.direction=v.direction;
DROP VIEW IF EXISTS v_nebulosity;
CREATE VIEW v_nebulosity AS
SELECT v.turn_id,v.direction,v.source,ROUND(SUM(v.value*d.weight)/SUM(d.weight),1) nebulosity
FROM v_metric_values v JOIN metric_definitions d ON d.id=v.metric_id
JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity'
GROUP BY v.turn_id,v.direction,v.source;
DROP VIEW IF EXISTS v_turn_summary;
CREATE VIEW v_turn_summary AS
SELECT t.id turn_id,t.session_id,t.seq,t.started_at,t.task_type,t.outcome,s.repo_url,s.ticket,
MAX(CASE WHEN d.key='estimated_seconds' THEN v.value END) estimated_seconds,
MAX(CASE WHEN d.key='real_seconds' THEN v.value END) real_seconds,
MAX(CASE WHEN d.key='blocked_seconds' THEN v.value END) blocked_seconds,
MAX(CASE WHEN d.key='estimate_ratio' THEN v.value END) estimate_ratio,
MAX(CASE WHEN d.key='server_requests' THEN v.value END) server_requests,
MAX(CASE WHEN d.key='words_in' THEN v.value END) words_in,
MAX(CASE WHEN d.key='words_out' THEN v.value END) words_out,
MAX(CASE WHEN d.key='agents' THEN v.value END) agents,
MAX(CASE WHEN d.key='corrections' THEN v.value END) corrections,
(SELECT nebulosity FROM v_nebulosity n WHERE n.turn_id=t.id AND n.direction='u2a' AND n.source='model') u2a_model,
(SELECT nebulosity FROM v_nebulosity n WHERE n.turn_id=t.id AND n.direction='u2a' AND n.source='heuristic') u2a_heuristic,
(SELECT nebulosity FROM v_nebulosity n WHERE n.turn_id=t.id AND n.direction='a2u' AND n.source='model') a2u_model,
(SELECT nebulosity FROM v_nebulosity n WHERE n.turn_id=t.id AND n.direction='a2u' AND n.source='heuristic') a2u_heuristic
FROM turns t JOIN sessions s ON s.id=t.session_id
LEFT JOIN metric_values v ON v.turn_id=t.id
LEFT JOIN metric_definitions d ON d.id=v.metric_id GROUP BY t.id;
DROP VIEW IF EXISTS v_keyword_stats;
CREATE VIEW v_keyword_stats AS
SELECT k.word,COUNT(DISTINCT t.id) turns,ROUND(AVG(rs.value),1) avg_real_seconds,
ROUND(AVG(er.value),2) avg_estimate_ratio
FROM keywords k JOIN turn_keywords tk ON tk.keyword_id=k.id JOIN turns t ON t.id=tk.turn_id
LEFT JOIN metric_values rs ON rs.turn_id=t.id AND rs.metric_id=(SELECT id FROM metric_definitions WHERE key='real_seconds')
LEFT JOIN metric_values er ON er.turn_id=t.id AND er.metric_id=(SELECT id FROM metric_definitions WHERE key='estimate_ratio')
GROUP BY k.id;
DROP VIEW IF EXISTS v_bias;
CREATE VIEW v_bias AS
SELECT d.key metric,m.direction,COUNT(*) samples,
ROUND(AVG(m.value-h.value),1) avg_model_minus_heuristic
FROM metric_values m JOIN v_metric_values h
ON h.turn_id=m.turn_id AND h.metric_id=m.metric_id AND h.direction=m.direction AND h.source='heuristic'
JOIN metric_definitions d ON d.id=m.metric_id WHERE m.source='model'
GROUP BY d.key,m.direction;
"#;

pub(crate) fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

pub(crate) fn initialize(connection: &Connection) -> Result<()> {
    connection.execute_batch("PRAGMA foreign_keys=ON")?;
    connection.execute_batch(SCHEMA)?;
    seed(connection)
}

fn seed(connection: &Connection) -> Result<()> {
    connection.execute_batch(
        "DELETE FROM heuristic_weights WHERE metric_id IN(
             SELECT id FROM metric_definitions WHERE key='cost_usd' OR key LIKE 'tokens_%'
                OR key IN('tool_calls','files_changed','lines_added','lines_removed','tests_passed'));
         DELETE FROM heuristic_calibration WHERE metric_id IN(
             SELECT id FROM metric_definitions WHERE key='cost_usd' OR key LIKE 'tokens_%'
                OR key IN('tool_calls','files_changed','lines_added','lines_removed','tests_passed'));
         DELETE FROM metric_values WHERE metric_id IN(
             SELECT id FROM metric_definitions WHERE key='cost_usd' OR key LIKE 'tokens_%'
                OR key IN('tool_calls','files_changed','lines_added','lines_removed','tests_passed'));
         DELETE FROM metric_definitions WHERE key='cost_usd' OR key LIKE 'tokens_%'
             OR key IN('tool_calls','files_changed','lines_added','lines_removed','tests_passed');",
    )?;
    let metrics = [
        (
            "estimated_seconds",
            "Estimated time",
            "time",
            "s",
            "lower",
            "sum",
            0,
        ),
        ("real_seconds", "Real time", "time", "s", "lower", "sum", 0),
        (
            "blocked_seconds",
            "Blocked time (permission waits)",
            "time",
            "s",
            "lower",
            "sum",
            0,
        ),
        (
            "estimate_ratio",
            "Real / estimated",
            "time",
            "ratio",
            "lower",
            "avg",
            0,
        ),
        (
            "server_requests",
            "Server requests",
            "usage",
            "count",
            "lower",
            "sum",
            0,
        ),
        ("words_in", "Words in", "usage", "count", "lower", "sum", 0),
        (
            "words_out",
            "Words out",
            "usage",
            "count",
            "lower",
            "sum",
            0,
        ),
        (
            "agents",
            "Agents spawned",
            "usage",
            "count",
            "lower",
            "sum",
            0,
        ),
        (
            "clarifying_questions",
            "Clarifying questions",
            "interaction",
            "count",
            "lower",
            "sum",
            0,
        ),
        (
            "corrections",
            "Corrections of previous turn",
            "interaction",
            "count",
            "lower",
            "sum",
            0,
        ),
        (
            "unmarked_questions",
            "Questions without '?'",
            "language",
            "count",
            "lower",
            "sum",
            1,
        ),
    ];
    for (key, name, category, unit, better, aggregation, directional) in metrics {
        connection.execute(
            "INSERT INTO metric_definitions(key,name,category,unit,better,aggregation,directional)
             VALUES(?1,?2,?3,?4,?5,?6,?7)
             ON CONFLICT(key) DO UPDATE SET name=excluded.name,category=excluded.category,
             unit=excluded.unit,better=excluded.better,aggregation=excluded.aggregation,
             directional=excluded.directional",
            params![key, name, category, unit, better, aggregation, directional],
        )?;
    }
    connection.execute("INSERT INTO metric_definitions(key,name,category,unit,better,aggregation,directional,description)
        VALUES('nebulosity','Linguistic nebulosity','language','score_0_100','lower','weighted_avg',1,
        'Lack of clarity, precision or transparency; composite of its children')
        ON CONFLICT(key) DO UPDATE SET name=excluded.name,category=excluded.category,unit=excluded.unit,
        better=excluded.better,aggregation=excluded.aggregation,directional=excluded.directional,
        description=excluded.description",[])?;
    let parent = metric_id(connection, "nebulosity")?;
    let children = [
        ("ambiguity", "Ambiguity", "Multiple plausible readings"),
        (
            "idea_branching",
            "Idea branching",
            "Nested or parallel idea trees",
        ),
        (
            "idea_dispersion",
            "Idea dispersion",
            "Ideas without a common thread",
        ),
        (
            "context_gap",
            "Context gap",
            "Missing context needed to act",
        ),
        ("explanation_gap", "Explanation gap", "Missing explanations"),
        ("argument_gap", "Argument gap", "Holes in the reasoning"),
        ("lucidity_deficit", "Lucidity deficit", "Lack of lucidity"),
        (
            "structural_complexity",
            "Structural complexity",
            "Convoluted structure",
        ),
        ("opacity", "Opacity", "Hard to see through to the intent"),
        (
            "certainty_deficit",
            "Certainty deficit",
            "Lack of certainty",
        ),
    ];
    debug_assert_eq!(children.map(|child| child.0), KEYS);
    for (key, name, description) in children {
        connection.execute("INSERT INTO metric_definitions(key,name,category,unit,better,parent_id,aggregation,directional,description)
            VALUES(?1,?2,'language','score_0_100','lower',?3,'avg',1,?4)
            ON CONFLICT(key) DO UPDATE SET name=excluded.name,category=excluded.category,unit=excluded.unit,
            better=excluded.better,parent_id=excluded.parent_id,aggregation=excluded.aggregation,
            directional=excluded.directional,description=excluded.description",params![key,name,parent,description])?;
    }
    Ok(())
}

pub(crate) fn metric_id(connection: &Connection, key: &str) -> Result<i64> {
    Ok(connection.query_row(
        "SELECT id FROM metric_definitions WHERE key=?1",
        [key],
        |row| row.get(0),
    )?)
}

pub(crate) fn put_metric(
    connection: &Connection,
    turn: i64,
    key: &str,
    value: f64,
    source: &str,
    direction: Option<&str>,
) -> Result<()> {
    connection.execute("INSERT INTO metric_values(turn_id,metric_id,direction,source,value,recorded_at)VALUES(?1,?2,?3,?4,?5,?6)ON CONFLICT DO UPDATE SET value=excluded.value,recorded_at=excluded.recorded_at",params![turn,metric_id(connection,key)?,direction,source,value,timestamp()])?;
    Ok(())
}

pub(crate) fn get_metric(connection: &Connection, turn: i64, key: &str) -> Result<Option<f64>> {
    Ok(connection.query_row("SELECT v.value FROM metric_values v JOIN metric_definitions m ON m.id=v.metric_id WHERE v.turn_id=?1 AND m.key=?2 ORDER BY v.id DESC LIMIT 1",params![turn,key],|row|row.get(0)).optional()?)
}

/// Returns this turn's model u2a score and the mean of the latest 30% of
/// earlier model-scored turns in the same session.
pub(crate) fn u2a_drop(connection: &Connection, turn: i64) -> Result<(Option<f64>, Option<f64>)> {
    let session: String =
        connection.query_row("SELECT session_id FROM turns WHERE id=?1", [turn], |row| {
            row.get(0)
        })?;
    let now = connection
        .query_row(
            "SELECT nebulosity FROM v_nebulosity
             WHERE turn_id=?1 AND direction='u2a' AND source='model'",
            [turn],
            |row| row.get(0),
        )
        .optional()?;
    let mut statement = connection.prepare(
        "SELECT t.id FROM turns t
         JOIN v_nebulosity n ON n.turn_id=t.id AND n.direction='u2a' AND n.source='model'
         WHERE t.session_id=?1
           AND (t.started_at<(SELECT started_at FROM turns WHERE id=?2)
                OR (t.started_at=(SELECT started_at FROM turns WHERE id=?2) AND t.id<?2))
         ORDER BY t.started_at,t.id",
    )?;
    let previous = statement
        .query_map(params![session, turn], |row| row.get::<_, i64>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if now.is_none() || previous.is_empty() {
        return Ok((now, None));
    }
    let count = ((previous.len() as f64 * 0.3).ceil() as usize).max(1);
    let recent = &previous[previous.len() - count..];
    let placeholders = std::iter::repeat_n("?", recent.len())
        .collect::<Vec<_>>()
        .join(",");
    let baseline = connection.query_row(
        &format!(
            "SELECT AVG(nebulosity) FROM v_nebulosity
             WHERE direction='u2a' AND source='model' AND turn_id IN ({placeholders})"
        ),
        params_from_iter(recent.iter()),
        |row| row.get(0),
    )?;
    Ok((now, baseline))
}

pub(crate) fn open_turn(
    connection: &Connection,
    session: Option<&str>,
) -> Result<(i64, String, i64, String)> {
    let query = if session.is_some() {
        "SELECT id,session_id,seq,started_at FROM turns WHERE ended_at IS NULL AND session_id=?1 ORDER BY started_at DESC LIMIT 1"
    } else {
        "SELECT id,session_id,seq,started_at FROM turns WHERE ended_at IS NULL ORDER BY started_at DESC LIMIT 1"
    };
    let turn = if let Some(session) = session {
        connection
            .query_row(query, [session], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?
    } else {
        connection
            .query_row(query, [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .optional()?
    };
    turn.context("no open turn (UserPromptSubmit hook not installed or turn already closed)")
}

pub(crate) fn add_keywords(connection: &Connection, turn: i64, input: &str) -> Result<()> {
    for word in input
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
    {
        connection.execute("INSERT OR IGNORE INTO keywords(word)VALUES(?1)", [&word])?;
        let keyword: i64 =
            connection.query_row("SELECT id FROM keywords WHERE word=?1", [&word], |row| {
                row.get(0)
            })?;
        connection.execute(
            "INSERT OR IGNORE INTO turn_keywords VALUES(?1,?2)",
            params![turn, keyword],
        )?;
    }
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn memory_db() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        initialize(&connection).unwrap();
        connection
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

    #[test]
    fn metric_helpers_insert_read_and_update_values() {
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");

        assert_eq!(get_metric(&connection, turn, "words_in").unwrap(), None);
        put_metric(&connection, turn, "words_in", 3.0, "hook", None).unwrap();
        assert_eq!(
            get_metric(&connection, turn, "words_in").unwrap(),
            Some(3.0)
        );
        put_metric(&connection, turn, "words_in", 8.0, "hook", None).unwrap();
        assert_eq!(
            get_metric(&connection, turn, "words_in").unwrap(),
            Some(8.0)
        );
        assert!(metric_id(&connection, "does-not-exist").is_err());
        assert!(put_metric(&connection, turn, "does-not-exist", 1.0, "test", None).is_err());
    }

    #[test]
    fn open_filters_by_session_and_reports_missing_turn() {
        let connection = memory_db();
        let first = add_turn(&connection, "first", 1, "2026-01-01T00:00:00.000Z");
        let second = add_turn(&connection, "second", 3, "2026-01-02T00:00:00.000Z");

        assert_eq!(open_turn(&connection, Some("first")).unwrap().0, first);
        assert_eq!(open_turn(&connection, None).unwrap().0, second);
        connection
            .execute(
                "UPDATE turns SET ended_at=started_at WHERE id IN (?1,?2)",
                params![first, second],
            )
            .unwrap();
        assert!(
            open_turn(&connection, None)
                .unwrap_err()
                .to_string()
                .contains("no open turn")
        );
    }

    #[test]
    fn keywords_normalize_trim_and_deduplicate() {
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        add_keywords(&connection, turn, " Rust, sqlite, RUST, , SQLite ").unwrap();

        let words: String = connection
            .query_row(
                "SELECT group_concat(word, ',') FROM (SELECT word FROM keywords ORDER BY word)",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let links: i64 = connection
            .query_row("SELECT COUNT(*) FROM turn_keywords", [], |row| row.get(0))
            .unwrap();
        assert_eq!(words, "rust,sqlite");
        assert_eq!(links, 2);
    }

    #[test]
    fn u2a_drop_uses_id_to_break_equal_timestamp_ties() {
        let connection = memory_db();
        let timestamp = "2026-01-01T00:00:00.000Z";
        let earlier = add_turn(&connection, "s1", 1, timestamp);
        let current = add_turn(&connection, "s1", 2, timestamp);
        let later = add_turn(&connection, "s1", 3, timestamp);
        for (turn, value) in [(earlier, 10.0), (current, 30.0), (later, 90.0)] {
            for key in KEYS {
                put_metric(&connection, turn, key, value, "model", Some("u2a")).unwrap();
            }
        }

        assert_eq!(
            u2a_drop(&connection, current).unwrap(),
            (Some(30.0), Some(10.0))
        );
    }

    #[test]
    fn initialize_repairs_seed_metadata_without_losing_existing_values() {
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        put_metric(&connection, turn, "real_seconds", 42.0, "hook", None).unwrap();
        connection
            .execute(
                "UPDATE metric_definitions SET name='wrong',category='core',unit='count',aggregation='sum' WHERE key='real_seconds'",
                [],
            )
            .unwrap();

        initialize(&connection).unwrap();

        let metadata: (String, String, String, String) = connection
            .query_row(
                "SELECT name,category,unit,aggregation FROM metric_definitions WHERE key='real_seconds'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(
            metadata,
            ("Real time".into(), "time".into(), "s".into(), "sum".into())
        );
        assert_eq!(
            get_metric(&connection, turn, "real_seconds").unwrap(),
            Some(42.0)
        );
    }

    #[test]
    fn initialize_removes_obsolete_python_metrics_and_enforces_catalog_checks() {
        let connection = memory_db();
        connection
            .execute(
                "INSERT INTO metric_definitions(key,name,category,unit,better) VALUES('tokens_in','Tokens','usage','count','lower')",
                [],
            )
            .unwrap();
        initialize(&connection).unwrap();
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM metric_definitions WHERE key='tokens_in'",
                    [],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
        assert!(
            connection
                .execute(
                    "INSERT INTO metric_definitions(key,name,category,unit,better) VALUES('invalid','Invalid','test','count','sideways')",
                    [],
                )
                .is_err()
        );
    }

    #[test]
    fn initialize_adds_protocol_validation_to_existing_unconstrained_database() {
        let connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch(
                "CREATE TABLE metric_definitions(
                    id INTEGER PRIMARY KEY AUTOINCREMENT,key TEXT NOT NULL UNIQUE,
                    name TEXT NOT NULL,category TEXT NOT NULL,unit TEXT NOT NULL,
                    better TEXT NOT NULL,parent_id INTEGER,weight REAL NOT NULL DEFAULT 1,
                    aggregation TEXT NOT NULL DEFAULT 'sum',directional INTEGER NOT NULL DEFAULT 0,
                    description TEXT,active INTEGER NOT NULL DEFAULT 1);",
            )
            .unwrap();

        initialize(&connection).unwrap();

        assert!(
            connection
                .execute(
                    "INSERT INTO metric_definitions(key,name,category,unit,better) VALUES('invalid','Invalid','test','count','sideways')",
                    [],
                )
                .is_err()
        );
        let turn = add_turn(&connection, "s1", 1, "2026-01-01T00:00:00.000Z");
        assert!(
            connection
                .execute(
                    "INSERT INTO metric_values(turn_id,metric_id,direction,source,value,recorded_at)
                     VALUES(?1,(SELECT id FROM metric_definitions WHERE key='words_in'),'sideways','unknown',1,'2026-01-01T00:00:00Z')",
                    [turn],
                )
                .is_err()
        );
    }

    #[test]
    fn reporting_views_expose_complete_summary_keywords_and_bias() {
        let connection = memory_db();
        let turn = add_turn(&connection, "s1", 7, "2026-01-01T00:00:00.000Z");
        connection
            .execute(
                "UPDATE sessions SET repo_url='owner/repo',ticket='NEB-7' WHERE id='s1'",
                [],
            )
            .unwrap();
        connection
            .execute(
                "UPDATE turns SET task_type='coding',outcome='completed' WHERE id=?1",
                [turn],
            )
            .unwrap();
        add_keywords(&connection, turn, "rust").unwrap();
        put_metric(&connection, turn, "estimated_seconds", 20.0, "model", None).unwrap();
        put_metric(&connection, turn, "real_seconds", 30.0, "hook", None).unwrap();
        put_metric(&connection, turn, "estimate_ratio", 1.5, "hook", None).unwrap();
        for key in KEYS {
            put_metric(&connection, turn, key, 20.0, "model", Some("u2a")).unwrap();
            put_metric(&connection, turn, key, 30.0, "heuristic", Some("u2a")).unwrap();
        }

        let summary: (i64, String, String, Option<f64>, Option<f64>) = connection
            .query_row(
                "SELECT seq,task_type,ticket,u2a_model,u2a_heuristic FROM v_turn_summary WHERE turn_id=?1",
                [turn],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            )
            .unwrap();
        assert_eq!(
            summary,
            (7, "coding".into(), "NEB-7".into(), Some(20.0), Some(30.0))
        );
        let keyword: (i64, f64, f64) = connection
            .query_row(
                "SELECT turns,avg_real_seconds,avg_estimate_ratio FROM v_keyword_stats WHERE word='rust'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(keyword, (1, 30.0, 1.5));
        let bias: (i64, f64) = connection
            .query_row(
                "SELECT samples,avg_model_minus_heuristic FROM v_bias WHERE metric='ambiguity' AND direction='u2a'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(bias, (1, -10.0));
    }
}
