use crate::{cli::ExportFormat, i18n::Localizer};
use anyhow::{Result, anyhow};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ReportDocument {
    pub(crate) format_version: u8,
    pub(crate) title: String,
    pub(crate) lang: String,
    pub(crate) generated_at: String,
    pub(crate) scope: Option<String>,
    pub(crate) filters_label: String,
    pub(crate) filters: Vec<String>,
    pub(crate) sections: Vec<ReportSection>,
}

impl ReportDocument {
    pub(crate) fn new(
        title: String,
        lang: String,
        scope: Option<String>,
        filters_label: String,
        filters: Vec<String>,
        sections: Vec<ReportSection>,
    ) -> Self {
        Self {
            format_version: 2,
            title,
            lang,
            generated_at: Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true),
            scope,
            filters_label,
            filters,
            sections,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(crate) enum ReportSection {
    Intro(IntroSection),
    Summary(SummarySection),
    Keywords(KeywordsSection),
    Profile(ProfileSection),
    Recommendations(RecommendationsSection),
}

impl ReportSection {
    fn title(&self) -> &str {
        match self {
            Self::Intro(s) => &s.title,
            Self::Summary(s) => &s.title,
            Self::Keywords(s) => &s.title,
            Self::Profile(s) => &s.title,
            Self::Recommendations(s) => &s.title,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct IntroSection {
    pub(crate) title: String,
    pub(crate) paragraphs: Vec<String>,
    pub(crate) scale_title: String,
    pub(crate) scale_headers: Vec<String>,
    pub(crate) scale: Vec<Vec<String>>,
    pub(crate) read_title: String,
    pub(crate) read_items: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub(crate) struct SummaryData {
    pub(crate) turns: usize,
    pub(crate) sessions: usize,
    pub(crate) real_seconds: f64,
    pub(crate) median_turn_seconds: Option<f64>,
    pub(crate) estimate_ratio: Option<f64>,
    pub(crate) requests: i64,
    pub(crate) agents: i64,
    pub(crate) words_in: i64,
    pub(crate) words_out: i64,
    pub(crate) u2a: Option<f64>,
    pub(crate) u2a_heuristic: Option<f64>,
    pub(crate) a2u: Option<f64>,
    pub(crate) a2u_heuristic: Option<f64>,
    pub(crate) corrections: i64,
    pub(crate) u2a_first: Option<f64>,
    pub(crate) u2a_second: Option<f64>,
    pub(crate) a2u_first: Option<f64>,
    pub(crate) a2u_second: Option<f64>,
    pub(crate) ratio_first: Option<f64>,
    pub(crate) ratio_second: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct BiasItem {
    pub(crate) metric: String,
    pub(crate) label: String,
    pub(crate) direction: String,
    pub(crate) delta: f64,
    pub(crate) samples: i64,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SummarySection {
    pub(crate) title: String,
    pub(crate) items: Vec<String>,
    pub(crate) data: SummaryData,
    pub(crate) bias: Vec<BiasItem>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeywordItem {
    pub(crate) word: String,
    pub(crate) turns: i64,
    pub(crate) avg_seconds: Option<f64>,
    pub(crate) avg_time: String,
    pub(crate) estimate_ratio: Option<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct KeywordsSection {
    pub(crate) title: String,
    pub(crate) headers: Vec<String>,
    pub(crate) rows: Vec<KeywordItem>,
    pub(crate) notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProfileMetric {
    pub(crate) key: String,
    pub(crate) metric: String,
    pub(crate) average: f64,
    pub(crate) first: Option<f64>,
    pub(crate) second: Option<f64>,
    pub(crate) trend: String,
    pub(crate) weak: bool,
    pub(crate) interpretation: String,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct SwotQuadrant {
    pub(crate) quadrant: String,
    pub(crate) label: String,
    pub(crate) items: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct DirectionProfile {
    pub(crate) direction: String,
    pub(crate) title: String,
    pub(crate) meta: String,
    pub(crate) headers: Vec<String>,
    pub(crate) rows: Vec<ProfileMetric>,
    pub(crate) swot_title: String,
    pub(crate) swot: Vec<SwotQuadrant>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct ProfileSection {
    pub(crate) title: String,
    pub(crate) weak_tag: String,
    pub(crate) weak_line: f64,
    pub(crate) profiles: Vec<DirectionProfile>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RecommendationGroup {
    pub(crate) group: String,
    pub(crate) title: String,
    pub(crate) ordered: bool,
    pub(crate) items: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct RecommendationsSection {
    pub(crate) title: String,
    pub(crate) groups: Vec<RecommendationGroup>,
    pub(crate) note: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OutputMode {
    Preferred,
    LegacyExport,
}

pub(crate) enum ReportOutput {
    Stdout(Vec<u8>),
    File(PathBuf),
}

pub(crate) fn output_report(
    report: &ReportDocument,
    format: ExportFormat,
    output: Option<PathBuf>,
    reports_dir: &Path,
    mode: OutputMode,
    localizer: &Localizer,
) -> Result<ReportOutput> {
    let bytes = render(report, format)?;
    let target = match (output, mode, format) {
        (Some(path), _, _) => Some((path, true)),
        (None, OutputMode::LegacyExport, _) => Some((default_local_path(format, 0), false)),
        (None, OutputMode::Preferred, ExportFormat::Html | ExportFormat::Pdf) => {
            fs::create_dir_all(reports_dir)?;
            Some((default_report_path(reports_dir, report, format, 0), false))
        }
        (None, OutputMode::Preferred, _) => None,
    };
    let Some((path, explicit)) = target else {
        return Ok(ReportOutput::Stdout(bytes));
    };
    match create_new(&path, &bytes) {
        Ok(()) => Ok(ReportOutput::File(path)),
        Err(error) if !explicit && error.kind() == io::ErrorKind::AlreadyExists => {
            for suffix in 1..1000 {
                let candidate = match mode {
                    OutputMode::LegacyExport => default_local_path(format, suffix),
                    OutputMode::Preferred => {
                        default_report_path(reports_dir, report, format, suffix)
                    }
                };
                match create_new(&candidate, &bytes) {
                    Ok(()) => return Ok(ReportOutput::File(candidate)),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(error) => return Err(export_error(localizer, &candidate, error)),
                }
            }
            Err(anyhow!(localizer.format(
                "report.export_failed",
                &[
                    ("path", path.display().to_string()),
                    ("error", localizer.text("report.export_collision"))
                ]
            )))
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            Err(anyhow!(localizer.format(
                "report.export_exists",
                &[("path", path.display().to_string())]
            )))
        }
        Err(error) => Err(export_error(localizer, &path, error)),
    }
}

pub(crate) fn render(report: &ReportDocument, format: ExportFormat) -> Result<Vec<u8>> {
    Ok(match format {
        ExportFormat::Md => render_markdown(report).into_bytes(),
        ExportFormat::Html => render_html(report).into_bytes(),
        ExportFormat::Json => {
            let mut bytes = serde_json::to_vec_pretty(report)?;
            bytes.push(b'\n');
            bytes
        }
        ExportFormat::Csv => render_csv(report).into_bytes(),
        ExportFormat::Pdf => render_pdf(&render_markdown(report)),
    })
}

fn create_new(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    if let Err(error) = file.write_all(bytes).and_then(|_| file.flush()) {
        drop(file);
        let _ = fs::remove_file(path);
        return Err(error);
    }
    Ok(())
}

fn export_error(localizer: &Localizer, path: &Path, error: io::Error) -> anyhow::Error {
    anyhow!(localizer.format(
        "report.export_failed",
        &[
            ("path", path.display().to_string()),
            ("error", error.to_string())
        ]
    ))
}

fn report_name(report: &ReportDocument, format: ExportFormat, suffix: usize) -> String {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let scope = report
        .scope
        .as_deref()
        .map_or(String::new(), |scope| format!("-{scope}"));
    let suffix = if suffix == 0 {
        String::new()
    } else {
        format!("-{suffix}")
    };
    format!(
        "report-{timestamp}-{}{scope}{suffix}.{}",
        report.lang,
        format.extension()
    )
}

fn default_local_path(format: ExportFormat, suffix: usize) -> PathBuf {
    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let suffix = if suffix == 0 {
        String::new()
    } else {
        format!("-{suffix}")
    };
    PathBuf::from(format!(
        "nebular-report-{timestamp}{suffix}.{}",
        format.extension()
    ))
}

fn default_report_path(
    reports_dir: &Path,
    report: &ReportDocument,
    format: ExportFormat,
    suffix: usize,
) -> PathBuf {
    reports_dir.join(report_name(report, format, suffix))
}

pub(crate) fn render_markdown(report: &ReportDocument) -> String {
    let mut out = vec![format!("# {}", report.title), String::new()];
    if !report.filters.is_empty() {
        out.extend([
            format!("_{} {}_", report.filters_label, report.filters.join(", ")),
            String::new(),
        ]);
    }
    for (index, section) in report.sections.iter().enumerate() {
        out.extend([
            format!("## {}. {}", index + 1, section.title()),
            String::new(),
        ]);
        match section {
            ReportSection::Intro(s) => {
                for p in &s.paragraphs {
                    out.extend([p.clone(), String::new()]);
                }
                out.extend([s.scale_title.clone(), String::new()]);
                out.extend(markdown_table(&s.scale_headers, &s.scale));
                out.extend([String::new(), s.read_title.clone()]);
                out.extend(s.read_items.iter().map(|x| format!("- {x}")));
            }
            ReportSection::Summary(s) => out.extend(s.items.iter().map(|x| format!("- {x}"))),
            ReportSection::Keywords(s) => {
                if s.rows.is_empty() {
                    out.extend(s.notes.clone());
                } else {
                    let rows = s
                        .rows
                        .iter()
                        .map(|r| {
                            vec![
                                format!("`{}`", r.word),
                                r.turns.to_string(),
                                r.avg_time.clone(),
                                format_optional(r.estimate_ratio, 2),
                            ]
                        })
                        .collect::<Vec<_>>();
                    out.extend(markdown_table(&s.headers, &rows));
                    out.push(String::new());
                    out.extend(s.notes.iter().map(|x| format!("- {x}")));
                }
            }
            ReportSection::Profile(s) => {
                for (pi, p) in s.profiles.iter().enumerate() {
                    out.extend([
                        format!("### {}", p.title),
                        String::new(),
                        format!("_{}_", p.meta),
                        String::new(),
                    ]);
                    let rows = p
                        .rows
                        .iter()
                        .map(|r| {
                            vec![
                                r.metric.clone(),
                                format!("{:.1}", r.average),
                                r.trend.clone(),
                                if r.weak {
                                    format!(
                                        "{} · {}",
                                        s.weak_tag,
                                        lowercase_first(&r.interpretation)
                                    )
                                } else {
                                    r.interpretation.clone()
                                },
                            ]
                        })
                        .collect::<Vec<_>>();
                    out.extend(markdown_table(&p.headers, &rows));
                    out.extend([String::new(), p.swot_title.clone()]);
                    out.extend(
                        p.swot
                            .iter()
                            .map(|q| format!("- {} {}", q.label, q.items.join(" "))),
                    );
                    if pi + 1 < s.profiles.len() {
                        out.push(String::new());
                    }
                }
            }
            ReportSection::Recommendations(s) => {
                if let Some(note) = &s.note {
                    out.extend([format!("_{note}_"), String::new()]);
                }
                for (gi, g) in s.groups.iter().enumerate() {
                    out.push(format!("**{}**", g.title));
                    out.extend(g.items.iter().enumerate().map(|(i, x)| {
                        if g.ordered {
                            format!("{}. {x}", i + 1)
                        } else {
                            format!("- {x}")
                        }
                    }));
                    if gi + 1 < s.groups.len() {
                        out.push(String::new());
                    }
                }
            }
        }
        out.push(String::new());
    }
    while out.last().is_some_and(String::is_empty) {
        out.pop();
    }
    out.join("\n") + "\n"
}

fn markdown_table(headers: &[String], rows: &[Vec<String>]) -> Vec<String> {
    let mut out = vec![
        format!("| {} |", headers.join(" | ")),
        format!(
            "|{}|",
            headers.iter().map(|_| "---").collect::<Vec<_>>().join("|")
        ),
    ];
    out.extend(rows.iter().map(|r| format!("| {} |", r.join(" | "))));
    out
}

const HTML_CSS: &str = r#":root{--bg:#f6f7f9;--panel:#fff;--fg:#1d2330;--muted:#5d6678;--line:#dfe3ea;--track:#e9ecf1;--bar:#8a97ad;--weak:#c4483e;--good:#2f7d4f;--bad:#b23a30;color-scheme:light}@media(prefers-color-scheme:dark){:root{--bg:#12151b;--panel:#1a1f28;--fg:#e6e9ef;--muted:#9aa3b5;--line:#2c3340;--track:#262c37;--bar:#6c7890;--weak:#e0675c;--good:#5cc28a;--bad:#ef7d72;color-scheme:dark}}*{box-sizing:border-box}body{margin:0;background:var(--bg);color:var(--fg);font:15px/1.55 system-ui,-apple-system,"Segoe UI",sans-serif}.wrap{max-width:72rem;margin:auto;padding:2rem max(16px,env(safe-area-inset-right)) 3rem;display:flex;flex-direction:column;gap:2rem}h1{font:400 2rem/1.2 Georgia,serif;margin:0}h2{font-size:1.15rem;margin:0;display:flex;gap:.5rem}h2 .n,.muted{color:var(--muted)}h3{font-size:1rem;margin:0}.grid,.tiles,.swot{display:grid;grid-template-columns:repeat(auto-fit,minmax(min(100%,18rem),1fr));gap:1rem}.card,.tile{background:var(--panel);border:1px solid var(--line);border-radius:10px;padding:1.1rem;display:flex;flex-direction:column;gap:.8rem}.tile .big{font:400 1.7rem/1.2 Georgia,serif}.scroll{overflow-x:auto}table{width:100%;border-collapse:collapse;font-variant-numeric:tabular-nums}th,td{text-align:left;padding:.45rem .6rem;border-bottom:1px solid var(--line);vertical-align:top}th{font-size:.8rem;color:var(--muted)}.barcell{display:flex;align-items:center;gap:.6rem;min-width:9rem}.val{width:2.8rem;text-align:right}.track{position:relative;flex:1;height:8px;border-radius:4px;background:var(--track)}.fill{position:absolute;inset:0 auto 0 0;border-radius:4px;background:var(--bar)}.fill.weak{background:var(--weak)}.mark{position:absolute;top:-3px;bottom:-3px;width:1px;background:var(--muted)}.pill{display:inline-block;font-size:.75rem;padding:0 .4rem;margin-right:.4rem;border:1px solid var(--weak);border-radius:999px;color:var(--weak)}.good{color:var(--good)}.bad{color:var(--bad)}.q{border:1px solid var(--line);border-radius:8px;padding:.8rem}.q h4{margin:0;font-size:.72rem;text-transform:uppercase;color:var(--muted)}ul,ol{margin:0;padding-left:1.2rem}code{background:var(--track);padding:0 .3em;border-radius:4px}section{display:flex;flex-direction:column;gap:1rem}footer{color:var(--muted);font-size:.8rem}"#;

fn render_html(report: &ReportDocument) -> String {
    let scale_max = report
        .sections
        .iter()
        .filter_map(|s| match s {
            ReportSection::Profile(p) => Some(
                p.profiles
                    .iter()
                    .flat_map(|p| p.rows.iter())
                    .map(|r| r.average)
                    .fold(40.0_f64, f64::max),
            ),
            _ => None,
        })
        .fold(40.0_f64, f64::max);
    let mut parts = vec![format!("<h1>{}</h1>", html(&report.title))];
    if !report.filters.is_empty() {
        parts.push(format!(
            "<p class=\"muted\">{} {}</p>",
            html(&report.filters_label),
            html(&report.filters.join(", "))
        ));
    }
    for (index, section) in report.sections.iter().enumerate() {
        let (kind, body) = match section {
            ReportSection::Intro(s) => (
                "intro",
                format!(
                    "<div class=\"grid\"><div class=\"card\">{}<h3>{}</h3><ul>{}</ul></div><div class=\"card\"><h3>{}</h3>{}</div></div>",
                    s.paragraphs
                        .iter()
                        .map(|p| format!("<p>{}</p>", inline_html(p)))
                        .collect::<String>(),
                    inline_html(&s.read_title),
                    s.read_items
                        .iter()
                        .map(|x| format!("<li>{}</li>", inline_html(x)))
                        .collect::<String>(),
                    inline_html(&s.scale_title),
                    html_table(&s.scale_headers, &s.scale)
                ),
            ),
            ReportSection::Summary(s) => {
                let tiles = [("u2a", s.data.u2a, s.data.u2a_heuristic), ("a2u", s.data.a2u, s.data.a2u_heuristic)].into_iter().map(|(label, value, heuristic)| format!("<div class=\"tile\"><span class=\"muted\">{label}</span><span class=\"big\">{}</span><span class=\"muted\">h {}</span></div>", format_optional(value, 2), format_optional(heuristic, 2))).collect::<String>();
                (
                    "summary",
                    format!(
                        "<div class=\"tiles\">{tiles}</div><div class=\"card\"><ul>{}</ul></div>",
                        s.items
                            .iter()
                            .map(|x| format!("<li>{}</li>", inline_html(x)))
                            .collect::<String>()
                    ),
                )
            }
            ReportSection::Keywords(s) => {
                let body = if s.rows.is_empty() {
                    format!("<p class=\"muted\">{}</p>", html(&s.notes.join(" ")))
                } else {
                    let top = s.rows.iter().map(|r| r.turns).max().unwrap_or(1) as f64;
                    let rows = s
                        .rows
                        .iter()
                        .map(|r| {
                            vec![
                                format!("<code>{}</code>", html(&r.word)),
                                bar(r.turns as f64, top, false, None),
                                html(&r.avg_time),
                                html(&format_optional(r.estimate_ratio, 2)),
                            ]
                        })
                        .collect::<Vec<_>>();
                    format!(
                        "<div class=\"card\">{}<ul class=\"muted\">{}</ul></div>",
                        html_table_raw(&s.headers, &rows),
                        s.notes
                            .iter()
                            .map(|x| format!("<li>{}</li>", inline_html(x)))
                            .collect::<String>()
                    )
                };
                ("keywords", body)
            }
            ReportSection::Profile(s) => {
                let body = s.profiles.iter().map(|p| { let rows = p.rows.iter().map(|r| { let class = match (r.first, r.second) { (Some(a), Some(b)) if b < a => "good", (Some(a), Some(b)) if b > a => "bad", _ => "" }; vec![html(&r.metric), bar(r.average, scale_max, r.weak, Some(s.weak_line)), format!("<span class=\"{class}\">{}</span>", html(&r.trend)), format!("{}{}", if r.weak { format!("<span class=\"pill\">{}</span>", html(&strip_markdown(&s.weak_tag))) } else { String::new() }, html(&r.interpretation))] }).collect::<Vec<_>>(); let swot = p.swot.iter().map(|q| format!("<div class=\"q\"><h4>{}</h4><ul>{}</ul></div>", html(strip_markdown(&q.label).trim_end_matches(':')), q.items.iter().map(|x| format!("<li>{}</li>", html(x))).collect::<String>())).collect::<String>(); format!("<div class=\"card\"><h3>{}</h3><p class=\"muted\">{}</p>{}<h3>{}</h3><div class=\"swot\">{swot}</div></div>", html(&p.title), html(&p.meta), html_table_raw(&p.headers, &rows), html(&strip_markdown(&p.swot_title))) }).collect::<String>();
                ("profile", body)
            }
            ReportSection::Recommendations(s) => {
                let note = s.note.as_ref().map_or(String::new(), |n| {
                    format!("<p class=\"muted\">{}</p>", html(n))
                });
                let cards = s
                    .groups
                    .iter()
                    .map(|g| {
                        let tag = if g.ordered { "ol" } else { "ul" };
                        format!(
                            "<div class=\"card\"><h3>{}</h3><{tag}>{}</{tag}></div>",
                            html(&g.title),
                            g.items
                                .iter()
                                .map(|x| format!("<li>{}</li>", inline_html(x)))
                                .collect::<String>()
                        )
                    })
                    .collect::<String>();
                (
                    "recommendations",
                    format!("{note}<div class=\"grid\">{cards}</div>"),
                )
            }
        };
        parts.push(format!(
            "<section id=\"{kind}\"><h2><span class=\"n\">{}</span>{}</h2>{body}</section>",
            index + 1,
            html(section.title())
        ));
    }
    parts.push(format!("<footer>{}</footer>", html(&report.generated_at)));
    format!(
        "<!doctype html>\n<html lang=\"{}\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width,initial-scale=1,viewport-fit=cover\"><title>{}</title><style>{HTML_CSS}</style></head><body><main class=\"wrap\">{}</main></body></html>\n",
        html(&report.lang),
        html(&report.title),
        parts.join("")
    )
}

fn bar(value: f64, maximum: f64, weak: bool, mark: Option<f64>) -> String {
    let width = (100.0 * value / maximum).clamp(0.0, 100.0);
    let marker = mark.map_or(String::new(), |m| {
        format!(
            "<div class=\"mark\" style=\"left:{:.1}%\"></div>",
            100.0 * m / maximum
        )
    });
    format!(
        "<div class=\"barcell\"><span class=\"val\">{value:.1}</span><div class=\"track\"><div class=\"fill{}\" style=\"width:{width:.1}%\"></div>{marker}</div></div>",
        if weak { " weak" } else { "" }
    )
}
fn html_table(headers: &[String], rows: &[Vec<String>]) -> String {
    html_table_raw(
        headers,
        &rows
            .iter()
            .map(|r| r.iter().map(|c| html(c)).collect())
            .collect::<Vec<Vec<_>>>(),
    )
}
fn html_table_raw(headers: &[String], rows: &[Vec<String>]) -> String {
    format!(
        "<div class=\"scroll\"><table><thead><tr>{}</tr></thead><tbody>{}</tbody></table></div>",
        headers
            .iter()
            .map(|h| format!("<th>{}</th>", html(h)))
            .collect::<String>(),
        rows.iter()
            .map(|row| format!(
                "<tr>{}</tr>",
                row.iter()
                    .map(|c| format!("<td>{c}</td>"))
                    .collect::<String>()
            ))
            .collect::<String>()
    )
}
fn html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}
fn inline_html(value: &str) -> String {
    let escaped = html(value);
    let mut out = String::new();
    let mut chars = escaped.chars().peekable();
    let mut bold = false;
    let mut italic = false;
    let mut code = false;
    while let Some(c) = chars.next() {
        if c == '*' && chars.peek() == Some(&'*') {
            chars.next();
            out.push_str(if bold { "</strong>" } else { "<strong>" });
            bold = !bold;
        } else if c == '*' {
            out.push_str(if italic { "</em>" } else { "<em>" });
            italic = !italic;
        } else if c == '`' {
            out.push_str(if code { "</code>" } else { "<code>" });
            code = !code;
        } else {
            out.push(c);
        }
    }
    if code {
        out.push_str("</code>");
    }
    if italic {
        out.push_str("</em>");
    }
    if bold {
        out.push_str("</strong>");
    }
    out
}

fn render_csv(report: &ReportDocument) -> String {
    let mut output = String::from("section,direction,item,label,measure,value,text\n");
    for section in &report.sections {
        match section {
            ReportSection::Summary(s) => {
                let values = serde_json::to_value(&s.data).expect("summary serializes");
                if let Some(fields) = values.as_object() {
                    for (measure, value) in fields {
                        csv_row(
                            &mut output,
                            &["summary", "", "total", "", measure, &json_scalar(value), ""],
                        );
                    }
                }
                for b in &s.bias {
                    csv_row(
                        &mut output,
                        &[
                            "bias",
                            &b.direction,
                            &b.metric,
                            &b.label,
                            "delta",
                            &b.delta.to_string(),
                            "",
                        ],
                    );
                    csv_row(
                        &mut output,
                        &[
                            "bias",
                            &b.direction,
                            &b.metric,
                            &b.label,
                            "samples",
                            &b.samples.to_string(),
                            "",
                        ],
                    );
                }
            }
            ReportSection::Keywords(s) => {
                for r in &s.rows {
                    for (measure, value) in [
                        ("turns", Some(r.turns as f64)),
                        ("avg_seconds", r.avg_seconds),
                        ("estimate_ratio", r.estimate_ratio),
                    ] {
                        csv_row(
                            &mut output,
                            &[
                                "keywords",
                                "",
                                &r.word,
                                &r.word,
                                measure,
                                &value.map(|v| v.to_string()).unwrap_or_default(),
                                "",
                            ],
                        );
                    }
                }
            }
            ReportSection::Profile(s) => {
                for p in &s.profiles {
                    for r in &p.rows {
                        for (measure, value) in [
                            ("avg", Some(r.average)),
                            ("first", r.first),
                            ("second", r.second),
                        ] {
                            csv_row(
                                &mut output,
                                &[
                                    "profile",
                                    &p.direction,
                                    &r.key,
                                    &r.metric,
                                    measure,
                                    &value.map(|v| v.to_string()).unwrap_or_default(),
                                    "",
                                ],
                            );
                        }
                        csv_row(
                            &mut output,
                            &[
                                "profile",
                                &p.direction,
                                &r.key,
                                &r.metric,
                                "weak",
                                if r.weak { "1" } else { "0" },
                                &r.interpretation,
                            ],
                        );
                    }
                    for q in &p.swot {
                        for (i, item) in q.items.iter().enumerate() {
                            csv_row(
                                &mut output,
                                &[
                                    "swot",
                                    &p.direction,
                                    &q.quadrant,
                                    strip_markdown(&q.label).trim_end_matches(':'),
                                    "index",
                                    &(i + 1).to_string(),
                                    item,
                                ],
                            );
                        }
                    }
                }
            }
            ReportSection::Recommendations(s) => {
                for g in &s.groups {
                    for (i, item) in g.items.iter().enumerate() {
                        csv_row(
                            &mut output,
                            &[
                                "recommendations",
                                "",
                                &g.group,
                                &g.title,
                                "index",
                                &(i + 1).to_string(),
                                &strip_markdown(item),
                            ],
                        );
                    }
                }
            }
            ReportSection::Intro(_) => {}
        }
    }
    output
}

fn json_scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Null => String::new(),
        serde_json::Value::String(v) => v.clone(),
        other => other.to_string(),
    }
}
fn csv_row(output: &mut String, values: &[&str]) {
    output.push_str(
        &values
            .iter()
            .map(|value| {
                let dangerous = value
                    .trim_start_matches(char::is_whitespace)
                    .starts_with(['=', '+', '-', '@']);
                let safe = if dangerous {
                    format!("'{value}")
                } else {
                    (*value).to_owned()
                };
                format!("\"{}\"", safe.replace('"', "\"\""))
            })
            .collect::<Vec<_>>()
            .join(","),
    );
    output.push('\n');
}
fn format_optional(value: Option<f64>, digits: usize) -> String {
    value.map_or_else(|| "—".into(), |value| format!("{value:.digits$}"))
}
fn lowercase_first(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_lowercase().collect::<String>() + chars.as_str())
        .unwrap_or_default()
}
fn strip_markdown(value: &str) -> String {
    value.replace(['*', '`'], "")
}

fn render_pdf(markdown: &str) -> Vec<u8> {
    let mut lines = Vec::new();
    for raw in markdown.lines() {
        let plain = strip_markdown(raw.trim_start_matches('#').trim().trim_start_matches("- "))
            .replace('→', "->");
        if plain.is_empty() {
            lines.push(String::new());
        } else {
            lines.extend(wrap_text(&plain, 88));
        }
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    let pages = lines.chunks(50).collect::<Vec<_>>();
    let mut objects = vec![b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()];
    let kids = (0..pages.len())
        .map(|i| format!("{} 0 R", 4 + i * 2))
        .collect::<Vec<_>>()
        .join(" ");
    objects.push(format!("<< /Type /Pages /Kids [{kids}] /Count {} >>", pages.len()).into_bytes());
    objects.push(
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_vec(),
    );
    for (index, page) in pages.iter().enumerate() {
        let content_id = 5 + index * 2;
        objects.push(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 3 0 R >> >> /Contents {content_id} 0 R >>").into_bytes());
        let mut stream = b"BT\n/F1 10 Tf\n50 790 Td\n".to_vec();
        for (i, line) in page.iter().enumerate() {
            if i > 0 {
                stream.extend_from_slice(b"0 -14 Td\n");
            }
            stream.push(b'(');
            stream.extend(pdf_string(line));
            stream.extend_from_slice(b") Tj\n");
        }
        stream.extend_from_slice(b"ET\n");
        let mut object = format!("<< /Length {} >>\nstream\n", stream.len()).into_bytes();
        object.extend(stream);
        object.extend_from_slice(b"endstream");
        objects.push(object);
    }
    pdf_file(objects)
}
fn wrap_text(value: &str, width: usize) -> Vec<String> {
    let mut output = Vec::new();
    let mut line = String::new();
    for word in value.split_whitespace() {
        if !line.is_empty() && line.chars().count() + word.chars().count() + 1 > width {
            output.push(line);
            line = String::new();
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    output.push(line);
    output
}
fn pdf_string(value: &str) -> Vec<u8> {
    let mut output = Vec::new();
    for c in value.chars() {
        let byte = match c {
            '\\' | '(' | ')' => {
                output.push(b'\\');
                c as u8
            }
            '€' => 0x80,
            '“' => 0x93,
            '”' => 0x94,
            '‘' => 0x91,
            '’' => 0x92,
            '–' => 0x96,
            '—' => 0x97,
            '•' => 0x95,
            '…' => 0x85,
            c if (c as u32) <= 0xff => c as u8,
            _ => b'?',
        };
        output.push(byte);
    }
    output
}
fn pdf_file(objects: Vec<Vec<u8>>) -> Vec<u8> {
    let mut output = b"%PDF-1.4\n%\xE2\xE3\xCF\xD3\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(output.len());
        output.extend_from_slice(format!("{} 0 obj\n", i + 1).as_bytes());
        output.extend_from_slice(object);
        output.extend_from_slice(b"\nendobj\n");
    }
    let xref = output.len();
    output.extend_from_slice(format!("xref\n0 {}\n", objects.len() + 1).as_bytes());
    output.extend_from_slice(b"0000000000 65535 f \n");
    for offset in offsets {
        output.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    output.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::i18n::{Lang, Localizer};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_PATH: AtomicU64 = AtomicU64::new(0);

    fn test_dir() -> PathBuf {
        let id = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "nebular-structured-report-{}-{id}",
            std::process::id()
        ))
    }
    fn report() -> ReportDocument {
        ReportDocument::new(
            "Report".into(),
            "en".into(),
            Some("u2a".into()),
            "Filters:".into(),
            vec!["last 7 days".into()],
            vec![
                ReportSection::Summary(SummarySection {
                    title: "Summary".into(),
                    items: vec!["**Turns:** 4".into()],
                    data: SummaryData {
                        turns: 4,
                        u2a: Some(12.5),
                        ..Default::default()
                    },
                    bias: vec![],
                }),
                ReportSection::Recommendations(RecommendationsSection {
                    title: "Recommendations".into(),
                    groups: vec![RecommendationGroup {
                        group: "you".into(),
                        title: "For you".into(),
                        ordered: true,
                        items: vec!["=be precise".into()],
                    }],
                    note: None,
                }),
            ],
        )
    }
    #[test]
    fn every_renderer_uses_semantic_model() {
        let report = report();
        let md = String::from_utf8(render(&report, ExportFormat::Md).unwrap()).unwrap();
        let html = String::from_utf8(render(&report, ExportFormat::Html).unwrap()).unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&render(&report, ExportFormat::Json).unwrap()).unwrap();
        let json_bytes = render(&report, ExportFormat::Json).unwrap();
        let csv = String::from_utf8(render(&report, ExportFormat::Csv).unwrap()).unwrap();
        let pdf = render(&report, ExportFormat::Pdf).unwrap();
        assert!(md.contains("## 1. Summary"));
        assert!(html.contains("prefers-color-scheme:dark") && html.contains("class=\"tiles\""));
        assert_eq!(json["sections"][0]["type"], "summary");
        assert_eq!(json["sections"][0]["data"]["turns"], 4);
        assert!(json_bytes.ends_with(b"\n"));
        assert!(csv.starts_with("section,direction,item,label,measure,value,text\n"));
        assert!(csv.contains("\"recommendations\",\"\",\"you\""));
        assert!(csv.contains("'=be precise"));
        assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
    }

    #[test]
    fn text_exports_preserve_identifiers_and_neutralize_spreadsheet_formulas() {
        let mut csv = String::new();
        csv_row(&mut csv, &["  \t=SUM(A1:A2)", "safe_value"]);
        assert!(csv.starts_with("\"'  \t=SUM(A1:A2)\""));
        assert_eq!(strip_markdown("use `safe_name`"), "use safe_name");
        assert_eq!(inline_html("*Average*"), "<em>Average</em>");
    }

    #[test]
    fn pdf_encodes_common_report_punctuation_in_windows_1252() {
        assert_eq!(pdf_string("—–…•"), vec![0x97, 0x96, 0x85, 0x95]);
    }

    #[test]
    fn preferred_and_legacy_output_modes_choose_expected_destinations() {
        let report = report();
        let localizer = Localizer::load(Lang::En).unwrap();
        let dir = test_dir();
        let stdout = output_report(
            &report,
            ExportFormat::Json,
            None,
            &dir,
            OutputMode::Preferred,
            &localizer,
        )
        .unwrap();
        assert!(matches!(stdout, ReportOutput::Stdout(bytes) if bytes.starts_with(b"{")));

        let html = output_report(
            &report,
            ExportFormat::Html,
            None,
            &dir,
            OutputMode::Preferred,
            &localizer,
        )
        .unwrap();
        let ReportOutput::File(path) = html else {
            panic!("html must be a file")
        };
        assert!(path.starts_with(&dir));
        assert!(
            path.extension()
                .is_some_and(|extension| extension == "html")
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn explicit_output_never_overwrites() {
        let report = report();
        let localizer = Localizer::load(Lang::En).unwrap();
        let dir = test_dir();
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("report.md");
        fs::write(&path, "keep").unwrap();
        let error = output_report(
            &report,
            ExportFormat::Md,
            Some(path.clone()),
            &dir,
            OutputMode::Preferred,
            &localizer,
        )
        .err()
        .expect("existing output must fail");
        assert!(error.to_string().contains("refusing to overwrite"));
        assert_eq!(fs::read_to_string(path).unwrap(), "keep");
        fs::remove_dir_all(dir).unwrap();
    }
}
