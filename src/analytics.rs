use crate::{
    app::App,
    cli::{ExportFormat, Scope},
    db::u2a_drop,
    export::{
        self, BiasItem, DirectionProfile, IntroSection, KeywordItem, KeywordsSection, OutputMode,
        ProfileMetric, ProfileSection, RecommendationGroup, RecommendationsSection, ReportDocument,
        ReportOutput, ReportSection, SummaryData, SummarySection, SwotQuadrant,
    },
    i18n::Localizer,
};
use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, params, params_from_iter, types::Value};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::PathBuf,
};

const WEAK: f64 = 20.0;
const STRONG: f64 = 10.0;
const BIAS_LIMIT: f64 = -3.0;

#[derive(Clone, Debug)]
struct TurnSummary {
    id: i64,
    session: String,
    real: Option<f64>,
    ratio: Option<f64>,
    requests: Option<f64>,
    words_in: Option<f64>,
    words_out: Option<f64>,
    agents: Option<f64>,
    corrections: Option<f64>,
    u2a_model: Option<f64>,
    u2a_heuristic: Option<f64>,
    a2u_model: Option<f64>,
    a2u_heuristic: Option<f64>,
}

#[derive(Clone, Debug)]
struct ProfileRow {
    key: String,
    average: f64,
    first: Option<f64>,
    second: Option<f64>,
    samples: usize,
}

#[derive(Clone, Debug)]
struct BiasRow {
    metric: String,
    direction: String,
    samples: i64,
    delta: f64,
}

pub(crate) fn report(
    app: &App,
    connection: &Connection,
    scope: Option<Scope>,
    days: Option<i64>,
    keyword: Option<String>,
    repo: Option<String>,
    format: Option<ExportFormat>,
    export_format: Option<ExportFormat>,
    output: Option<PathBuf>,
    recs: Option<String>,
    localizer: &Localizer,
) -> Result<()> {
    if let (Some(format), Some(export)) = (format, export_format)
        && format != export
    {
        bail!(localizer.text("report.format_conflict"));
    }
    let recommendations = parse_recommendations(recs.as_deref(), scope, localizer)?;
    let Some(report) = build_report(
        connection,
        scope,
        days,
        keyword.clone(),
        repo.clone(),
        recommendations,
        localizer,
    )?
    else {
        println!("{}", localizer.text("report.no_data"));
        return Ok(());
    };
    let selected = format.or(export_format).unwrap_or(ExportFormat::Md);
    let mode = if format.is_some() || export_format.is_none() {
        OutputMode::Preferred
    } else {
        OutputMode::LegacyExport
    };
    match export::output_report(
        &report,
        selected,
        output,
        &app.reports_dir(),
        mode,
        localizer,
    )? {
        ReportOutput::Stdout(bytes) => io::stdout().write_all(&bytes)?,
        ReportOutput::File(path) => println!(
            "{}",
            localizer.format("report.exported", &[("path", path.display().to_string())])
        ),
    }
    Ok(())
}

pub(crate) fn tips(
    connection: &Connection,
    share: f64,
    both: bool,
    all_history: bool,
    session: Option<&str>,
    localizer: &Localizer,
) -> Result<()> {
    println!(
        "{}",
        render_tips(connection, share, both, all_history, session, localizer)?
    );
    Ok(())
}

pub(crate) fn estimate(
    connection: &Connection,
    keywords: &str,
    localizer: &Localizer,
) -> Result<()> {
    println!("{}", render_estimate(connection, keywords, localizer)?);
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecommendationInput {
    #[serde(default)]
    you: Vec<String>,
    #[serde(default)]
    me: Vec<String>,
    #[serde(default)]
    both: Vec<String>,
}

fn parse_recommendations(
    raw: Option<&str>,
    scope: Option<Scope>,
    localizer: &Localizer,
) -> Result<RecommendationsSection> {
    let title = localizer.text("report.section_recommendations");
    let Some(raw) = raw else {
        return Ok(RecommendationsSection {
            title,
            groups: Vec::new(),
            note: Some(localizer.text("report.recommendations_missing")),
        });
    };
    let source = if PathBuf::from(raw).is_file() {
        fs::read_to_string(raw).with_context(|| {
            localizer.format(
                "report.recommendations_invalid",
                &[("error", raw.to_owned())],
            )
        })?
    } else {
        raw.to_owned()
    };
    let input: RecommendationInput = serde_json::from_str(&source).map_err(|error| {
        anyhow::anyhow!(localizer.format(
            "report.recommendations_invalid",
            &[("error", error.to_string())],
        ))
    })?;
    let mut groups = Vec::new();
    if scope != Some(Scope::A2u) && !input.you.is_empty() {
        groups.push(RecommendationGroup {
            group: "you".into(),
            title: localizer.text("report.for_you_plain"),
            ordered: true,
            items: input.you,
        });
    }
    if scope != Some(Scope::U2a) && !input.me.is_empty() {
        groups.push(RecommendationGroup {
            group: "me".into(),
            title: localizer.text("report.for_me_plain"),
            ordered: true,
            items: input.me,
        });
    }
    if !input.both.is_empty() {
        groups.push(RecommendationGroup {
            group: "both".into(),
            title: localizer.text("report.for_both_plain"),
            ordered: false,
            items: input.both,
        });
    }
    Ok(RecommendationsSection {
        title,
        groups,
        note: None,
    })
}

fn build_report(
    connection: &Connection,
    scope: Option<Scope>,
    days: Option<i64>,
    keyword: Option<String>,
    repo: Option<String>,
    recommendations: RecommendationsSection,
    localizer: &Localizer,
) -> Result<Option<ReportDocument>> {
    let rows = filtered_turns(connection, days, keyword.as_deref(), repo.as_deref())?;
    if rows.is_empty() {
        return Ok(None);
    }
    set_report_turns(connection, rows.iter().map(|row| row.id))?;
    let scopes = match scope {
        Some(value) => vec![scope_name(value)],
        None => vec!["u2a", "a2u"],
    };
    let bias = bias_rows(connection)?;
    let profiles = scopes
        .iter()
        .map(|direction| Ok((*direction, profile_rows(connection, direction, "model")?)))
        .collect::<Result<BTreeMap<_, _>>>()?;
    let unmarked = unmarked_questions(connection)?;
    let corrections = total(rows.iter().map(|row| row.corrections));

    let mut filters = Vec::new();
    if let Some(days) = days {
        filters.push(localizer.format("report.filter_days", &[("n", days.to_string())]));
    }
    if let Some(keyword) = keyword {
        filters.push(localizer.format(
            "report.filter_keyword",
            &[("keyword", keyword.trim().to_lowercase())],
        ));
    }
    if let Some(repo) = repo {
        filters.push(localizer.format("report.filter_repo", &[("repo", repo)]));
    }
    let mut sections = Vec::new();
    if scope.is_none() {
        sections.push(ReportSection::Intro(intro_document(localizer)));
        sections.push(ReportSection::Summary(summary_document(
            &rows,
            &bias,
            corrections,
            localizer,
        )));
        sections.push(ReportSection::Keywords(keyword_document(
            connection, localizer,
        )?));
    }
    let mut direction_profiles = Vec::new();
    for direction in &scopes {
        direction_profiles.push(profile_document(
            direction,
            profiles.get(direction).expect("profile exists"),
            bias.iter()
                .filter(|row| row.direction == *direction)
                .collect::<Vec<_>>()
                .as_slice(),
            if *direction == "u2a" {
                Some(unmarked)
            } else {
                None
            },
            if *direction == "u2a" {
                corrections
            } else {
                0.0
            },
            localizer,
        ));
    }
    sections.push(ReportSection::Profile(ProfileSection {
        title: localizer.text("report.section_profile"),
        weak_tag: localizer.text("report.weak"),
        weak_line: WEAK,
        profiles: direction_profiles,
    }));
    sections.push(ReportSection::Recommendations(recommendations));
    Ok(Some(ReportDocument::new(
        localizer.text("report.title"),
        localizer.lang().code().into(),
        scope.map(scope_name).map(str::to_owned),
        localizer.text("report.filters_label"),
        filters,
        sections,
    )))
}

#[cfg(test)]
fn render_report(
    connection: &Connection,
    scope: Option<Scope>,
    days: Option<i64>,
    keyword: Option<String>,
    repo: Option<String>,
    localizer: &Localizer,
) -> Result<String> {
    let recs = parse_recommendations(None, scope, localizer)?;
    Ok(
        build_report(connection, scope, days, keyword, repo, recs, localizer)?.map_or_else(
            || localizer.text("report.no_data"),
            |report| export::render_markdown(&report),
        ),
    )
}

fn intro_document(localizer: &Localizer) -> IntroSection {
    IntroSection {
        title: localizer.text("report.section_intro"),
        paragraphs: vec![
            localizer.text("report.intro_what"),
            localizer.text("report.intro_directions"),
        ],
        scale_title: localizer.text("report.intro_scale"),
        scale_headers: vec![
            localizer.text("report.range"),
            localizer.text("report.meaning"),
        ],
        scale: vec![
            vec!["0-20".into(), localizer.text("report.scale_crisp")],
            vec!["21-40".into(), localizer.text("report.scale_minor")],
            vec!["41-60".into(), localizer.text("report.scale_assumptions")],
            vec!["61-80".into(), localizer.text("report.scale_misread")],
            vec!["81-100".into(), localizer.text("report.scale_blocked")],
        ],
        read_title: localizer.text("report.intro_read"),
        read_items: vec![
            localizer.text("report.read_average"),
            localizer.text("report.read_trend"),
            localizer.format("report.read_weak", &[("weak", WEAK.to_string())]),
            localizer.text("report.read_heuristic"),
            localizer.text("report.read_bias"),
        ],
    }
}

fn summary_document(
    rows: &[TurnSummary],
    bias: &[BiasRow],
    corrections: f64,
    localizer: &Localizer,
) -> SummarySection {
    let sessions = rows
        .iter()
        .map(|row| row.session.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let mut reals = rows.iter().filter_map(|row| row.real).collect::<Vec<_>>();
    let half = rows.len() / 2;
    let (first, second) = if half >= 3 {
        (&rows[..half], &rows[half..])
    } else {
        (&[][..], &[][..])
    };
    let data = SummaryData {
        turns: rows.len(),
        sessions,
        real_seconds: total(rows.iter().map(|r| r.real)),
        median_turn_seconds: median(&mut reals),
        estimate_ratio: average(rows.iter().map(|r| r.ratio)),
        requests: total(rows.iter().map(|r| r.requests)) as i64,
        agents: total(rows.iter().map(|r| r.agents)) as i64,
        words_in: total(rows.iter().map(|r| r.words_in)) as i64,
        words_out: total(rows.iter().map(|r| r.words_out)) as i64,
        u2a: average(rows.iter().map(|r| r.u2a_model)),
        u2a_heuristic: average(rows.iter().map(|r| r.u2a_heuristic)),
        a2u: average(rows.iter().map(|r| r.a2u_model)),
        a2u_heuristic: average(rows.iter().map(|r| r.a2u_heuristic)),
        corrections: corrections as i64,
        u2a_first: average(first.iter().map(|r| r.u2a_model)),
        u2a_second: average(second.iter().map(|r| r.u2a_model)),
        a2u_first: average(first.iter().map(|r| r.a2u_model)),
        a2u_second: average(second.iter().map(|r| r.a2u_model)),
        ratio_first: average(first.iter().map(|r| r.ratio)),
        ratio_second: average(second.iter().map(|r| r.ratio)),
    };
    let items = summary(rows, bias, corrections, localizer)
        .into_iter()
        .map(|item| item.strip_prefix("- ").unwrap_or(&item).to_owned())
        .collect();
    let bias = bias
        .iter()
        .take(5)
        .map(|row| BiasItem {
            metric: row.metric.clone(),
            label: metric_name(localizer, &row.metric),
            direction: row.direction.clone(),
            delta: row.delta,
            samples: row.samples,
        })
        .collect();
    SummarySection {
        title: localizer.text("report.section_summary"),
        items,
        data,
        bias,
    }
}

fn keyword_document(connection: &Connection, localizer: &Localizer) -> Result<KeywordsSection> {
    let mut statement = connection.prepare("SELECT k.word,COUNT(*) turns,AVG(s.real_seconds),AVG(s.estimate_ratio) FROM turn_keywords tk JOIN keywords k ON k.id=tk.keyword_id JOIN v_turn_summary s ON s.turn_id=tk.turn_id WHERE tk.turn_id IN (SELECT id FROM temp.report_turns) GROUP BY k.word ORDER BY turns DESC,k.word LIMIT 10")?;
    let rows = statement
        .query_map([], |row| {
            Ok(KeywordItem {
                word: row.get(0)?,
                turns: row.get(1)?,
                avg_seconds: row.get(2)?,
                avg_time: fmt_seconds(row.get(2)?),
                estimate_ratio: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut notes = Vec::new();
    if rows.is_empty() {
        notes.push(localizer.text("report.keyword_none"));
    } else {
        notes.push(
            localizer.format(
                "report.keyword_frequent",
                &[(
                    "items",
                    join(
                        localizer,
                        rows.iter()
                            .take(2)
                            .map(|r| format!("`{}` ({})", r.word, r.turns)),
                    ),
                )],
            ),
        );
        let mut slow = rows
            .iter()
            .filter(|r| r.turns >= 3 && r.avg_seconds.is_some())
            .collect::<Vec<_>>();
        slow.sort_by(|a, b| {
            b.avg_seconds
                .unwrap_or_default()
                .total_cmp(&a.avg_seconds.unwrap_or_default())
        });
        if !slow.is_empty() {
            notes.push(
                localizer.format(
                    "report.keyword_slowest",
                    &[(
                        "items",
                        join(
                            localizer,
                            slow.into_iter()
                                .take(2)
                                .map(|r| format!("`{}` ({})", r.word, r.avg_time)),
                        ),
                    )],
                ),
            );
        }
    }
    Ok(KeywordsSection {
        title: localizer.text("report.section_keywords"),
        headers: vec![
            localizer.text("report.keyword"),
            localizer.text("report.turns"),
            localizer.text("report.average_time"),
            localizer.text("report.estimate_ratio"),
        ],
        rows,
        notes,
    })
}

fn profile_document(
    direction: &str,
    profile: &[ProfileRow],
    bias: &[&BiasRow],
    unmarked: Option<(f64, i64)>,
    corrections: f64,
    localizer: &Localizer,
) -> DirectionProfile {
    let rows = profile
        .iter()
        .map(|row| ProfileMetric {
            key: row.key.clone(),
            metric: metric_name(localizer, &row.key),
            average: row.average,
            first: row.first,
            second: row.second,
            trend: trend_cell(row, localizer),
            weak: row.average >= WEAK,
            interpretation: capitalize(
                &localizer.text(&format!("interpretation.{direction}.{}", row.key)),
            ),
        })
        .collect();
    let swot_lines = swot(direction, profile, bias, unmarked, corrections, localizer);
    let mut quadrants = Vec::new();
    for kind in ["strengths", "weaknesses", "opportunities", "threats"] {
        let label = localizer.text(&format!("report.{kind}"));
        let prefix = format!("- {label} ");
        let fallback = localizer.text("common.none");
        let text = swot_lines
            .iter()
            .find_map(|line| line.strip_prefix(&prefix))
            .unwrap_or(&fallback)
            .to_owned();
        quadrants.push(SwotQuadrant {
            quadrant: kind.into(),
            label,
            items: vec![text],
        });
    }
    DirectionProfile {
        direction: direction.into(),
        title: capitalize(&localizer.text(&format!("direction.{direction}"))),
        meta: localizer.format(
            "report.profile_meta",
            &[(
                "turns",
                profile
                    .iter()
                    .map(|r| r.samples)
                    .max()
                    .unwrap_or(0)
                    .to_string(),
            )],
        ),
        headers: vec![
            localizer.text("report.submetric"),
            localizer.text("report.average"),
            localizer.text("report.trend_heading"),
            localizer.text("report.interpretation"),
        ],
        rows,
        swot_title: localizer.text("report.swot"),
        swot: quadrants,
    }
}

fn filtered_turns(
    connection: &Connection,
    days: Option<i64>,
    keyword: Option<&str>,
    repo: Option<&str>,
) -> Result<Vec<TurnSummary>> {
    let mut sql = "SELECT turn_id,session_id,real_seconds,estimate_ratio,server_requests,words_in,words_out,agents,corrections,u2a_model,u2a_heuristic,a2u_model,a2u_heuristic FROM v_turn_summary WHERE 1=1".to_owned();
    let mut values = Vec::new();
    if let Some(days) = days {
        sql.push_str(" AND datetime(started_at)>=datetime('now', ?)");
        values.push(Value::Text(format!("-{days} days")));
    }
    if let Some(repo) = repo {
        sql.push_str(" AND repo_url LIKE ?");
        values.push(Value::Text(format!("%{}%", repo.trim())));
    }
    if let Some(keyword) = keyword {
        sql.push_str(" AND turn_id IN (SELECT tk.turn_id FROM turn_keywords tk JOIN keywords k ON k.id=tk.keyword_id WHERE k.word=?)");
        values.push(Value::Text(keyword.trim().to_lowercase()));
    }
    sql.push_str(" ORDER BY started_at");
    let mut statement = connection.prepare(&sql)?;
    Ok(statement
        .query_map(params_from_iter(values), |row| {
            Ok(TurnSummary {
                id: row.get(0)?,
                session: row.get(1)?,
                real: row.get(2)?,
                ratio: row.get(3)?,
                requests: row.get(4)?,
                words_in: row.get(5)?,
                words_out: row.get(6)?,
                agents: row.get(7)?,
                corrections: row.get(8)?,
                u2a_model: row.get(9)?,
                u2a_heuristic: row.get(10)?,
                a2u_model: row.get(11)?,
                a2u_heuristic: row.get(12)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn set_report_turns(connection: &Connection, ids: impl Iterator<Item = i64>) -> Result<()> {
    connection.execute_batch(
        "CREATE TEMP TABLE IF NOT EXISTS report_turns(id INTEGER PRIMARY KEY);DELETE FROM temp.report_turns;",
    )?;
    let mut insert = connection.prepare("INSERT INTO temp.report_turns VALUES (?1)")?;
    for id in ids {
        insert.execute([id])?;
    }
    Ok(())
}

fn summary(
    rows: &[TurnSummary],
    bias: &[BiasRow],
    corrections: f64,
    localizer: &Localizer,
) -> Vec<String> {
    let sessions = rows
        .iter()
        .map(|row| row.session.as_str())
        .collect::<BTreeSet<_>>()
        .len();
    let mut reals = rows.iter().filter_map(|row| row.real).collect::<Vec<_>>();
    let mut lines = vec![
        format!(
            "- {}",
            localizer.format(
                "report.volume",
                &[
                    ("turns", rows.len().to_string()),
                    ("sessions", sessions.to_string()),
                    (
                        "real",
                        fmt_seconds(Some(total(rows.iter().map(|row| row.real))))
                    ),
                    ("median", fmt_seconds(median(&mut reals))),
                    (
                        "ratio",
                        fmt_number(average(rows.iter().map(|row| row.ratio)), 2)
                    ),
                ],
            )
        ),
        format!(
            "- {}",
            localizer.format(
                "report.usage",
                &[
                    (
                        "requests",
                        format!("{:.0}", total(rows.iter().map(|row| row.requests)))
                    ),
                    (
                        "agents",
                        format!("{:.0}", total(rows.iter().map(|row| row.agents)))
                    ),
                    (
                        "words_in",
                        fmt_count(total(rows.iter().map(|row| row.words_in)))
                    ),
                    (
                        "words_out",
                        fmt_count(total(rows.iter().map(|row| row.words_out)))
                    ),
                ],
            )
        ),
        format!(
            "- {}",
            localizer.format(
                "report.nebulosity",
                &[
                    (
                        "u2a",
                        fmt_number(average(rows.iter().map(|row| row.u2a_model)), 2)
                    ),
                    (
                        "u2a_h",
                        fmt_number(average(rows.iter().map(|row| row.u2a_heuristic)), 2)
                    ),
                    (
                        "a2u",
                        fmt_number(average(rows.iter().map(|row| row.a2u_model)), 2)
                    ),
                    (
                        "a2u_h",
                        fmt_number(average(rows.iter().map(|row| row.a2u_heuristic)), 2)
                    ),
                    ("corrections", format!("{corrections:.0}")),
                ],
            )
        ),
    ];
    let half = rows.len() / 2;
    if half >= 3 {
        let (first, second) = rows.split_at(half);
        lines.push(format!(
            "- {}",
            localizer.format(
                "report.trend",
                &[
                    (
                        "u2a_first",
                        fmt_number(average(first.iter().map(|r| r.u2a_model)), 1)
                    ),
                    (
                        "u2a_second",
                        fmt_number(average(second.iter().map(|r| r.u2a_model)), 1)
                    ),
                    (
                        "a2u_first",
                        fmt_number(average(first.iter().map(|r| r.a2u_model)), 1)
                    ),
                    (
                        "a2u_second",
                        fmt_number(average(second.iter().map(|r| r.a2u_model)), 1)
                    ),
                    (
                        "ratio_first",
                        fmt_number(average(first.iter().map(|r| r.ratio)), 2)
                    ),
                    (
                        "ratio_second",
                        fmt_number(average(second.iter().map(|r| r.ratio)), 2)
                    ),
                ],
            )
        ));
    } else {
        lines.push(format!("- {}", localizer.text("report.trend_short")));
    }
    if !bias.is_empty() {
        let items = bias.iter().take(5).map(|row| {
            localizer.format(
                "report.bias_item",
                &[
                    ("metric", metric_name(localizer, &row.metric)),
                    ("direction", row.direction.clone()),
                    ("delta", format!("{:+.1}", row.delta)),
                    ("samples", row.samples.to_string()),
                ],
            )
        });
        lines.push(format!(
            "- {}",
            localizer.format("report.bias", &[("items", join(localizer, items))])
        ));
    }
    lines
}

fn profile_rows(connection: &Connection, direction: &str, source: &str) -> Result<Vec<ProfileRow>> {
    let mut statement = connection.prepare(
        "SELECT d.key,v.value FROM v_metric_values v JOIN metric_definitions d ON d.id=v.metric_id JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity' JOIN turns t ON t.id=v.turn_id WHERE v.direction=?1 AND v.source=?2 AND v.turn_id IN (SELECT id FROM temp.report_turns) ORDER BY t.started_at",
    )?;
    let mut grouped: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for row in statement.query_map(params![direction, source], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, f64>(1)?))
    })? {
        let (key, value) = row?;
        grouped.entry(key).or_default().push(value);
    }
    let mut output = grouped
        .into_iter()
        .map(|(key, values)| {
            let half = values.len() / 2;
            let (first, second) = if half >= 3 {
                (
                    Some(values[..half].iter().sum::<f64>() / half as f64),
                    Some(values[half..].iter().sum::<f64>() / (values.len() - half) as f64),
                )
            } else {
                (None, None)
            };
            ProfileRow {
                key,
                average: values.iter().sum::<f64>() / values.len() as f64,
                first,
                second,
                samples: values.len(),
            }
        })
        .collect::<Vec<_>>();
    output.sort_by(|a, b| b.average.total_cmp(&a.average));
    Ok(output)
}

fn bias_rows(connection: &Connection) -> Result<Vec<BiasRow>> {
    let mut statement = connection.prepare(
        "SELECT d.key,m.direction,COUNT(*),AVG(m.value-h.value) FROM metric_values m JOIN v_metric_values h ON h.turn_id=m.turn_id AND h.metric_id=m.metric_id AND h.direction=m.direction AND h.source='heuristic' JOIN metric_definitions d ON d.id=m.metric_id JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity' WHERE m.source='model' AND m.turn_id IN (SELECT id FROM temp.report_turns) GROUP BY d.key,m.direction ORDER BY AVG(m.value-h.value)",
    )?;
    Ok(statement
        .query_map([], |row| {
            Ok(BiasRow {
                metric: row.get(0)?,
                direction: row.get(1)?,
                samples: row.get(2)?,
                delta: row.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn unmarked_questions(connection: &Connection) -> Result<(f64, i64)> {
    Ok(connection.query_row(
        "SELECT COALESCE(SUM(v.value),0),COUNT(*) FROM metric_values v JOIN metric_definitions d ON d.id=v.metric_id WHERE d.key='unmarked_questions' AND v.direction='u2a' AND v.turn_id IN (SELECT id FROM temp.report_turns)",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?)
}

fn swot(
    direction: &str,
    profile: &[ProfileRow],
    bias: &[&BiasRow],
    unmarked: Option<(f64, i64)>,
    corrections: f64,
    localizer: &Localizer,
) -> Vec<String> {
    let crisp = profile
        .iter()
        .filter(|row| row.average < STRONG)
        .collect::<Vec<_>>();
    let improving_rows = profile
        .iter()
        .filter(|row| improving(row))
        .collect::<Vec<_>>();
    let weak = profile
        .iter()
        .filter(|row| row.average >= WEAK)
        .collect::<Vec<_>>();
    let mut strengths = Vec::new();
    if !crisp.is_empty() {
        strengths.push(localizer.format(
            "report.strength_crisp",
            &[
                ("strong", format!("{STRONG:.0}")),
                (
                    "items",
                    join(
                        localizer,
                        crisp.iter().rev().map(|row| named_score(localizer, row)),
                    ),
                ),
            ],
        ));
    } else if !profile.is_empty() {
        strengths.push(
            localizer.format(
                "report.strength_best",
                &[(
                    "items",
                    join(
                        localizer,
                        profile
                            .iter()
                            .rev()
                            .take(2)
                            .map(|row| named_score(localizer, row)),
                    ),
                )],
            ),
        );
    }
    if !improving_rows.is_empty() {
        strengths.push(localizer.format(
            "report.strength_improving",
            &[
                ("count", improving_rows.len().to_string()),
                ("total", profile.len().to_string()),
            ],
        ));
    }
    let mut weaknesses = vec![if weak.is_empty() {
        localizer.format("report.weakness_none", &[("weak", format!("{WEAK:.0}"))])
    } else {
        localizer.format(
            "report.weakness_list",
            &[
                ("weak", format!("{WEAK:.0}")),
                (
                    "items",
                    join(
                        localizer,
                        weak.iter().map(|row| named_score(localizer, row)),
                    ),
                ),
            ],
        )
    }];
    if let Some((count, turns)) = unmarked.filter(|(count, _)| *count > 0.0) {
        weaknesses.push(localizer.format(
            "report.weakness_unmarked",
            &[
                ("count", format!("{count:.0}")),
                ("turns", turns.to_string()),
            ],
        ));
    }
    let rising = weak
        .iter()
        .copied()
        .filter(|row| improving(row))
        .collect::<Vec<_>>();
    let mut opportunities = Vec::new();
    if !rising.is_empty() {
        opportunities.push(localizer.format(
            "report.opportunity_improving",
            &[(
                "items",
                join(
                    localizer,
                    rising.iter().map(|row| trend_score(localizer, row)),
                ),
            )],
        ));
    }
    if let Some(best) = improving_rows
        .iter()
        .filter(|row| row.average < WEAK)
        .max_by(|a, b| improvement(a).total_cmp(&improvement(b)))
    {
        opportunities.push(localizer.format(
            "report.opportunity_momentum",
            &[
                ("metric", metric_name(localizer, &best.key)),
                ("first", format!("{:.0}", best.first.unwrap_or_default())),
                ("second", format!("{:.0}", best.second.unwrap_or_default())),
            ],
        ));
    }
    let mut threats = profile
        .iter()
        .filter(|row| {
            row.average >= 15.0
                && row.first.is_some()
                && row.second.unwrap_or_default().round() >= row.first.unwrap_or_default().round()
        })
        .map(|row| {
            localizer.format(
                "report.threat_stalled",
                &[
                    ("metric", metric_name(localizer, &row.key)),
                    ("first", format!("{:.0}", row.first.unwrap_or_default())),
                    ("second", format!("{:.0}", row.second.unwrap_or_default())),
                ],
            )
        })
        .collect::<Vec<_>>();
    let biased = bias
        .iter()
        .filter(|row| row.delta <= BIAS_LIMIT)
        .collect::<Vec<_>>();
    if !biased.is_empty() {
        threats.push(localizer.format(
            &format!("report.threat_bias_{direction}"),
            &[(
                "items",
                join(
                    localizer,
                    biased.iter().map(|row| {
                        format!(
                            "{} ({:+.1})",
                            metric_name(localizer, &row.metric),
                            row.delta
                        )
                    }),
                ),
            )],
        ));
    }
    if corrections > 0.0 && !weak.is_empty() {
        threats.push(localizer.format(
            "report.threat_corrections",
            &[("count", format!("{corrections:.0}"))],
        ));
    }
    vec![
        localizer.text("report.swot"),
        swot_line("strengths", strengths, localizer),
        swot_line("weaknesses", weaknesses, localizer),
        swot_line("opportunities", opportunities, localizer),
        swot_line("threats", threats, localizer),
    ]
}

fn swot_line(kind: &str, items: Vec<String>, localizer: &Localizer) -> String {
    let text = if items.is_empty() {
        localizer.text("common.none")
    } else {
        items
            .into_iter()
            .map(|item| format!("{}.", capitalize(&item)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    format!("- {} {text}", localizer.text(&format!("report.{kind}")))
}

fn render_tips(
    connection: &Connection,
    share: f64,
    both: bool,
    all_history: bool,
    session: Option<&str>,
    localizer: &Localizer,
) -> Result<String> {
    let current = if let Some(session) = session {
        connection
            .query_row(
                "SELECT id,session_id FROM turns WHERE session_id=?1
                 ORDER BY (ended_at IS NULL) DESC,started_at DESC,id DESC LIMIT 1",
                [session],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
    } else {
        connection
            .query_row(
                "SELECT id,session_id FROM turns
                 ORDER BY (ended_at IS NULL) DESC,started_at DESC,id DESC LIMIT 1",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
            )
            .optional()?
    };
    let ids = if all_history {
        scored_turn_ids(connection, None)?
    } else if current.is_some() {
        scored_turn_ids(connection, current.as_ref().map(|row| row.1.as_str()))?
    } else {
        Vec::new()
    };
    if ids.is_empty() {
        return Ok(localizer.text("report.no_data"));
    }
    let share = share.clamp(0.3, 1.0);
    let window_count = ((ids.len() as f64 * share).ceil() as usize).max(1);
    let split = ids.len() - window_count;
    let (before, window) = ids.split_at(split);
    let last = ids.last().copied().into_iter().collect::<Vec<_>>();
    let mut output = Vec::new();
    if let Some((turn, _)) = current {
        let (now, baseline) = u2a_drop(connection, turn)?;
        if let (Some(now), Some(baseline)) = (now, baseline) {
            output.push(localizer.format(
                if now > baseline {
                    "tips.gate_on"
                } else {
                    "tips.gate_off"
                },
                &[
                    ("now", format!("{now:.1}")),
                    ("base", format!("{baseline:.1}")),
                ],
            ));
        }
    }
    output.push(localizer.format(
        if all_history {
            "tips.window_all"
        } else {
            "tips.window"
        },
        &[
            ("k", window_count.to_string()),
            ("n", ids.len().to_string()),
            (
                "pct",
                ((100.0 * window_count as f64 / ids.len() as f64).round() as usize).to_string(),
            ),
        ],
    ));
    let directions = if both {
        ["u2a", "a2u"].as_slice()
    } else {
        ["u2a"].as_slice()
    };
    for direction in directions {
        let window_values = tip_averages(connection, window, direction)?;
        let before_values = tip_averages(connection, before, direction)?;
        let last_values = tip_averages(connection, &last, direction)?;
        output.push(String::new());
        output.push(capitalize(
            &localizer.text(&format!("direction.{direction}")),
        ));
        output.push(format!(
            "  {:<26}{:>9}{:>9}{:>8}{:>9}",
            localizer.text("tips.head_metric"),
            localizer.text("tips.head_window"),
            localizer.text("tips.head_before"),
            localizer.text("tips.head_delta"),
            localizer.text("tips.head_last")
        ));
        let mut ranked = window_values.iter().collect::<Vec<_>>();
        ranked.sort_by(|left, right| right.1.total_cmp(left.1));
        for (key, value) in ranked {
            let before_value = before_values.get(key);
            let delta = before_value.map(|baseline| value - baseline);
            output.push(format!(
                "  {:<26}{:>9.1}{:>9}{:>8}{:>9}",
                metric_name(localizer, key),
                value,
                before_value.map_or_else(|| "—".into(), |v| format!("{v:.1}")),
                delta.map_or_else(|| "—".into(), |v| format!("{v:+.1}")),
                last_values
                    .get(key)
                    .map_or_else(|| "—".into(), |v| format!("{v:.1}")),
            ));
        }
        let window_overall = mean(window_values.values().copied());
        let before_overall = mean(before_values.values().copied());
        output.push(format!(
            "  {}",
            localizer.format(
                "tips.overall",
                &[
                    ("w", fmt_number(window_overall, 1)),
                    ("b", fmt_number(before_overall, 1)),
                ],
            )
        ));
    }
    Ok(output.join("\n"))
}

fn scored_turn_ids(connection: &Connection, session: Option<&str>) -> Result<Vec<i64>> {
    let (sql, values): (&str, Vec<Value>) = if let Some(session) = session {
        (
            "SELECT DISTINCT t.id FROM turns t
             JOIN metric_values v ON v.turn_id=t.id
             JOIN metric_definitions d ON d.id=v.metric_id
             JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity'
             WHERE v.direction='u2a' AND v.source='model' AND t.session_id=?1
             ORDER BY t.started_at,t.id",
            vec![session.to_owned().into()],
        )
    } else {
        (
            "SELECT DISTINCT t.id FROM turns t
             JOIN metric_values v ON v.turn_id=t.id
             JOIN metric_definitions d ON d.id=v.metric_id
             JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity'
             WHERE v.direction='u2a' AND v.source='model'
             ORDER BY t.started_at,t.id",
            Vec::new(),
        )
    };
    let mut statement = connection.prepare(sql)?;
    Ok(statement
        .query_map(params_from_iter(values), |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?)
}

fn tip_averages(
    connection: &Connection,
    turns: &[i64],
    direction: &str,
) -> Result<BTreeMap<String, f64>> {
    if turns.is_empty() {
        return Ok(BTreeMap::new());
    }
    let placeholders = std::iter::repeat_n("?", turns.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT d.key,AVG(v.value) FROM v_metric_values v
         JOIN metric_definitions d ON d.id=v.metric_id
         JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity'
         WHERE v.direction=?1 AND v.source='model'
           AND v.turn_id IN ({placeholders}) GROUP BY d.key"
    );
    let values = std::iter::once(Value::Text(direction.to_owned()))
        .chain(turns.iter().copied().map(Value::Integer));
    let mut statement = connection.prepare(&sql)?;
    Ok(statement
        .query_map(params_from_iter(values), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?)
}

fn mean(values: impl Iterator<Item = f64>) -> Option<f64> {
    let values = values.collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn render_estimate(
    connection: &Connection,
    keywords: &str,
    localizer: &Localizer,
) -> Result<String> {
    let words = keywords
        .split(',')
        .map(|word| word.trim().to_lowercase())
        .filter(|word| !word.is_empty())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if words.is_empty() {
        bail!(localizer.text("estimate.no_keywords"));
    }
    let placeholders = std::iter::repeat_n("?", words.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT t.id,COUNT(*) overlap,v.value FROM turns t JOIN turn_keywords tk ON tk.turn_id=t.id JOIN keywords k ON k.id=tk.keyword_id JOIN metric_values v ON v.turn_id=t.id AND v.metric_id=(SELECT id FROM metric_definitions WHERE key='real_seconds') WHERE k.word IN ({placeholders}) GROUP BY t.id"
    );
    let mut statement = connection.prepare(&sql)?;
    let matches = statement
        .query_map(params_from_iter(words.iter()), |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, f64>(1)?,
                row.get::<_, f64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if matches.is_empty() {
        return Ok(localizer.text("estimate.none"));
    }
    let weight = matches.iter().map(|(_, overlap, _)| overlap).sum::<f64>();
    let weighted = matches
        .iter()
        .map(|(_, overlap, real)| overlap * real)
        .sum::<f64>()
        / weight;
    let mut reals = matches.iter().map(|(_, _, real)| *real).collect::<Vec<_>>();
    reals.sort_by(f64::total_cmp);
    let p90 = reals[((reals.len() as f64 * 0.9) as usize).min(reals.len() - 1)];
    let ids = matches.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
    let ratio_sql = format!(
        "SELECT AVG(v.value) FROM metric_values v WHERE v.metric_id=(SELECT id FROM metric_definitions WHERE key='estimate_ratio') AND v.turn_id IN ({})",
        std::iter::repeat_n("?", ids.len())
            .collect::<Vec<_>>()
            .join(",")
    );
    let ratio: Option<f64> =
        connection.query_row(&ratio_sql, params_from_iter(ids), |row| row.get(0))?;
    let mut output = localizer.format(
        "estimate.line",
        &[
            ("turns", matches.len().to_string()),
            ("weighted", fmt_seconds(Some(weighted))),
            ("median", fmt_seconds(median(&mut reals))),
            ("p90", fmt_seconds(Some(p90))),
        ],
    );
    if let Some(ratio) = ratio {
        output.push_str(&localizer.format("estimate.ratio", &[("ratio", format!("{ratio:.2}"))]));
    }
    Ok(output)
}

fn scope_name(scope: Scope) -> &'static str {
    match scope {
        Scope::U2a => "u2a",
        Scope::A2u => "a2u",
    }
}

fn trend_cell(row: &ProfileRow, localizer: &Localizer) -> String {
    match (row.first, row.second) {
        (Some(first), Some(second)) => format!(
            "{:.0} → {:.0} {}",
            first,
            second,
            trend_label(first, second, localizer)
        ),
        _ => "—".into(),
    }
}

fn trend_label(first: f64, second: f64, localizer: &Localizer) -> String {
    let key = if second.round() < first.round() {
        "trend.better"
    } else if second.round() > first.round() {
        "trend.worse"
    } else {
        "trend.flat"
    };
    localizer.text(key)
}

fn improving(row: &ProfileRow) -> bool {
    matches!((row.first, row.second), (Some(first), Some(second)) if second.round() < first.round())
}

fn improvement(row: &&ProfileRow) -> f64 {
    row.first.unwrap_or_default() - row.second.unwrap_or_default()
}

fn named_score(localizer: &Localizer, row: &ProfileRow) -> String {
    format!("{} ({:.1})", metric_name(localizer, &row.key), row.average)
}

fn trend_score(localizer: &Localizer, row: &&ProfileRow) -> String {
    format!(
        "{} ({:.0} → {:.0})",
        metric_name(localizer, &row.key),
        row.first.unwrap_or_default(),
        row.second.unwrap_or_default()
    )
}

fn metric_name(localizer: &Localizer, key: &str) -> String {
    localizer.text(&format!("metric.{key}"))
}

fn total(values: impl Iterator<Item = Option<f64>>) -> f64 {
    let value = values.flatten().sum::<f64>();
    if value == 0.0 { 0.0 } else { value }
}

fn average(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let values = values.flatten().collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

fn median(values: &mut [f64]) -> Option<f64> {
    if values.is_empty() {
        return None;
    }
    values.sort_by(f64::total_cmp);
    let middle = values.len() / 2;
    Some(if values.len() % 2 == 0 {
        (values[middle - 1] + values[middle]) / 2.0
    } else {
        values[middle]
    })
}

fn fmt_seconds(value: Option<f64>) -> String {
    value.map_or_else(
        || "?".into(),
        |seconds| {
            let seconds = seconds as i64;
            if seconds >= 60 {
                format!("{}m{:02}s", seconds / 60, seconds % 60)
            } else {
                format!("{seconds}s")
            }
        },
    )
}

fn fmt_number(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.digits$}"))
}

fn fmt_count(value: f64) -> String {
    if value >= 1000.0 {
        format!("{:.1}k", value / 1000.0)
    } else {
        format!("{value:.0}")
    }
}

fn join(localizer: &Localizer, values: impl IntoIterator<Item = String>) -> String {
    values
        .into_iter()
        .collect::<Vec<_>>()
        .join(&localizer.text("common.list_separator"))
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, scoring::KEYS};

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        db::initialize(&connection).unwrap();
        connection
    }

    fn english() -> Localizer {
        Localizer::load(crate::i18n::Lang::En).unwrap()
    }

    fn add_turn(
        connection: &Connection,
        session: &str,
        repo: &str,
        sequence: i64,
        day: usize,
    ) -> i64 {
        connection.execute("INSERT OR IGNORE INTO sessions(id,started_at,repo_url)VALUES(?1,'2026-10-01T00:00:00Z',?2)", params![session, repo]).unwrap();
        connection
            .execute(
                "UPDATE sessions SET repo_url=?2 WHERE id=?1",
                params![session, repo],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO turns(session_id,seq,started_at,ended_at)VALUES(?1,?2,?3,?3)",
                params![session, sequence, format!("2026-10-{day:02}T00:00:00Z")],
            )
            .unwrap();
        connection.last_insert_rowid()
    }

    fn populate(connection: &Connection) {
        for index in 1..=8 {
            let turn = add_turn(
                connection,
                if index < 5 { "s1" } else { "s2" },
                if index % 2 == 0 {
                    "owner/nebula"
                } else {
                    "owner/other"
                },
                index as i64,
                index,
            );
            db::add_keywords(
                connection,
                turn,
                if index % 2 == 0 { "rust,cli" } else { "docs" },
            )
            .unwrap();
            for (key, value) in [
                ("real_seconds", 30.0 + index as f64),
                ("estimate_ratio", 1.0 + index as f64 / 10.0),
                ("server_requests", 2.0),
                ("words_in", 100.0),
                ("words_out", 200.0),
                ("agents", 1.0),
                ("corrections", 1.0),
            ] {
                db::put_metric(connection, turn, key, value, "hook", None).unwrap();
            }
            for key in KEYS {
                db::put_metric(
                    connection,
                    turn,
                    key,
                    10.0 + index as f64 * 3.0,
                    "model",
                    Some("u2a"),
                )
                .unwrap();
                db::put_metric(
                    connection,
                    turn,
                    key,
                    24.0 + index as f64 * 3.0,
                    "model",
                    Some("a2u"),
                )
                .unwrap();
                db::put_metric(
                    connection,
                    turn,
                    key,
                    20.0 + index as f64,
                    "heuristic",
                    Some("u2a"),
                )
                .unwrap();
                db::put_metric(
                    connection,
                    turn,
                    key,
                    25.0 + index as f64,
                    "heuristic",
                    Some("a2u"),
                )
                .unwrap();
            }
            db::put_metric(
                connection,
                turn,
                "unmarked_questions",
                1.0,
                "heuristic",
                Some("u2a"),
            )
            .unwrap();
        }
    }

    #[test]
    fn report_renders_all_sections_and_scoped_profile() {
        let connection = database();
        populate(&connection);
        let full = render_report(&connection, None, None, None, None, &english()).unwrap();
        assert!(full.contains("# Linguistic nebulosity report"));
        assert!(full.contains("## 1. Introduction"));
        assert!(full.contains("Keyword analysis"));
        assert!(full.contains("Analysis (SWOT)"));
        assert!(full.contains("Recommendations"));
        let scoped =
            render_report(&connection, Some(Scope::U2a), None, None, None, &english()).unwrap();
        assert!(!scoped.contains("Introduction"));
        assert!(scoped.contains("How you write (u2a)"));
        assert!(!scoped.contains("How I write (a2u)"));
    }

    #[test]
    fn analytics_output_is_fully_localized_in_spanish() {
        let connection = database();
        populate(&connection);
        let spanish = Localizer::load(crate::i18n::Lang::Es).unwrap();
        let report = render_report(&connection, None, None, None, None, &spanish).unwrap();
        let tips = render_tips(&connection, 0.3, false, false, None, &spanish).unwrap();
        let estimate = render_estimate(&connection, "rust", &spanish).unwrap();
        assert!(report.contains("# Reporte de nebulosidad lingüística"));
        assert!(report.contains("**Análisis (FODA)**"));
        assert!(report.contains("## 5. Recomendaciones"));
        assert!(tips.contains("ventana: últimos"));
        assert!(tips.contains("Sub-métrica"));
        assert!(estimate.contains("coinciden 4 turnos"));
    }

    #[test]
    fn report_filters_keyword_and_repository_across_analytics() {
        let connection = database();
        populate(&connection);
        let output = render_report(
            &connection,
            None,
            None,
            Some("RUST".into()),
            Some("nebula".into()),
            &english(),
        )
        .unwrap();
        assert!(
            output.contains("Filters: keyword rust, repo nebula"),
            "{output}"
        );
        assert!(output.contains("4 turns"));
        assert!(!output.contains("`docs`"));
        assert_eq!(
            filtered_turns(&connection, None, Some("missing"), None)
                .unwrap()
                .len(),
            0
        );
    }

    #[test]
    fn recommendations_validate_and_follow_report_scope() {
        let localizer = english();
        let all = parse_recommendations(
            Some(r#"{"you":["Clarify"],"me":["Lead"],"both":["Review"]}"#),
            None,
            &localizer,
        )
        .unwrap();
        assert_eq!(
            all.groups
                .iter()
                .map(|group| group.group.as_str())
                .collect::<Vec<_>>(),
            ["you", "me", "both"]
        );
        let scoped = parse_recommendations(
            Some(r#"{"you":["Clarify"],"me":["Lead"],"both":["Review"]}"#),
            Some(Scope::A2u),
            &localizer,
        )
        .unwrap();
        assert_eq!(
            scoped
                .groups
                .iter()
                .map(|group| group.group.as_str())
                .collect::<Vec<_>>(),
            ["me", "both"]
        );
        let file = std::env::temp_dir().join(format!(
            "nebula-recommendations-{}.json",
            std::process::id()
        ));
        fs::write(&file, r#"{"you":["From file"]}"#).unwrap();
        let from_file = parse_recommendations(file.to_str(), Some(Scope::U2a), &localizer).unwrap();
        fs::remove_file(file).unwrap();
        assert_eq!(from_file.groups[0].items, ["From file"]);
        assert!(
            parse_recommendations(Some(r#"{"you":"not-an-array"}"#), None, &localizer)
                .unwrap_err()
                .to_string()
                .contains("--recs must be JSON")
        );
        assert!(
            parse_recommendations(Some(r#"{"you":[],"unexpected":[]}"#), None, &localizer).is_err()
        );
        assert!(
            parse_recommendations(None, None, &localizer)
                .unwrap()
                .note
                .unwrap()
                .contains("No recommendations")
        );
    }

    #[test]
    fn structured_report_preserves_scope_and_recommendations() {
        let connection = database();
        populate(&connection);
        let localizer = english();
        let recs = parse_recommendations(
            Some(r#"{"you":["Clarify"],"me":["Lead"],"both":["Review"]}"#),
            Some(Scope::U2a),
            &localizer,
        )
        .unwrap();
        let report = build_report(
            &connection,
            Some(Scope::U2a),
            None,
            None,
            None,
            recs,
            &localizer,
        )
        .unwrap()
        .unwrap();
        assert_eq!(report.scope.as_deref(), Some("u2a"));
        assert_eq!(report.sections.len(), 2);
        let ReportSection::Profile(profile) = &report.sections[0] else {
            panic!("expected profile")
        };
        assert_eq!(profile.profiles.len(), 1);
        assert_eq!(profile.profiles[0].direction, "u2a");
        let ReportSection::Recommendations(recommendations) = &report.sections[1] else {
            panic!("expected recommendations")
        };
        assert_eq!(
            recommendations
                .groups
                .iter()
                .map(|group| group.group.as_str())
                .collect::<Vec<_>>(),
            ["you", "both"]
        );
    }

    #[test]
    fn day_filter_compares_rfc3339_timestamps_as_datetimes() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO sessions(id,started_at) VALUES('s','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection.execute(
            "INSERT INTO turns(session_id,seq,started_at,ended_at)
             VALUES('s',1,strftime('%Y-%m-%dT%H:%M:%SZ','now','-1 day','-1 second'),strftime('%Y-%m-%dT%H:%M:%SZ','now','-1 day','-1 second'))",
            [],
        ).unwrap();
        connection.execute(
            "INSERT INTO turns(session_id,seq,started_at,ended_at)
             VALUES('s',2,strftime('%Y-%m-%dT%H:%M:%SZ','now'),strftime('%Y-%m-%dT%H:%M:%SZ','now'))",
            [],
        ).unwrap();

        let rows = filtered_turns(&connection, Some(1), None, None).unwrap();
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn report_and_tips_handle_empty_data() {
        let connection = database();
        assert_eq!(
            render_report(&connection, None, None, None, None, &english()).unwrap(),
            "no data"
        );
        assert_eq!(
            render_tips(&connection, 0.3, false, false, None, &english()).unwrap(),
            "no data"
        );
    }

    #[test]
    fn tips_show_conversation_window_gate_and_both_directions() {
        let connection = database();
        populate(&connection);
        let output = render_tips(&connection, 0.1, true, false, Some("s2"), &english()).unwrap();
        assert!(output.contains("this turn's u2a"));
        assert!(output.contains("write a tip"));
        assert!(output.contains("latest 2 of 4 scored turns"));
        assert!(output.contains("How you write (u2a)"));
        assert!(output.contains("How I write (a2u)"));
        assert!(output.contains("Window"));
        assert!(output.contains("Before"));
        assert!(output.contains("Last"));
        assert!(output.contains("+"));
        assert_eq!(
            render_tips(&connection, 0.3, false, false, Some("missing"), &english()).unwrap(),
            "no data"
        );
        let history = render_tips(&connection, 0.3, false, true, None, &english()).unwrap();
        assert!(history.contains("latest 3 of 8 scored turns"));
    }

    #[test]
    fn estimate_uses_overlap_weighting_median_p90_and_ratio() {
        let connection = database();
        populate(&connection);
        let output = render_estimate(&connection, "rust, cli", &english()).unwrap();
        assert!(output.contains("matches 4 turns"));
        assert!(output.contains("weighted"));
        assert!(output.contains("median"));
        assert!(output.contains("p90"));
        assert!(output.contains("past estimate ratio"));
        assert_eq!(
            render_estimate(&connection, "unknown", &english()).unwrap(),
            "no historical turns match these keywords"
        );
        assert!(render_estimate(&connection, " , ", &english()).is_err());
    }

    #[test]
    fn format_helpers_cover_boundaries() {
        assert_eq!(fmt_seconds(None), "?");
        assert_eq!(fmt_seconds(Some(59.9)), "59s");
        assert_eq!(fmt_seconds(Some(61.0)), "1m01s");
        assert_eq!(fmt_count(999.0), "999");
        assert_eq!(fmt_count(1500.0), "1.5k");
        assert_eq!(median(&mut []), None);
        assert_eq!(median(&mut [1.0, 3.0, 2.0]), Some(2.0));
        assert_eq!(median(&mut [1.0, 3.0]), Some(2.0));
        assert!(total(std::iter::empty()).is_sign_positive());
    }
}
