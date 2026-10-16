use crate::{
    db::{metric_id, put_metric, timestamp},
    i18n::Localizer,
    scoring::{KEYS, heuristic},
};
use anyhow::Result;
use chrono::{DateTime, Utc};
use once_cell::sync::Lazy;
use regex::Regex;
use rusqlite::{Connection, params};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    env, fs,
    path::{Path, PathBuf},
};

const RIDGE_PENALTIES: [f64; 5] = [1., 10., 50., 200., 1_000.];
const FOLDS: usize = 5;

static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{L}0-9_']+").unwrap());
static CONCRETE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(`[^`]+`|/[\w.-]+/|\w+\.\w{1,5}\b|\b[A-Z]{2,}-\d+\b|\d+|https?://)").unwrap()
});
static LIST_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*([-*+]|\d+[.)])\s").unwrap());
static IMPERATIVE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)\b(haz|crea|agrega|añade|quita|elimina|remueve|cambia|ajusta|muestra|revisa|lee|corre|ejecuta|make|create|add|remove|delete|change|update|fix|show|run|read|check)\b",
    )
    .unwrap()
});
static QUESTION_START: Lazy<HashSet<&'static str>> = Lazy::new(|| {
    "que qué como cómo cual cuál cuales cuáles donde dónde cuando cuándo cuanto cuánto quien quién puedes podrías podrias podemos deberíamos deberiamos es está esta hay tienes sabes what how why which where when who whom whose should can could would will do does did is are was were have has may shall"
        .split_whitespace()
        .collect()
});
static QUESTION_PHRASE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(me entiendes|me explico|tiene sentido|makes? sense)\b|\b(verdad|cierto|right|correct)\s*$").unwrap()
});

const VAGUE: &str = "thing things stuff something somehow somewhere whatever etc it that this those these algo cosa cosas eso esto esa ese aquello lo la le así tal";
const HEDGES: &str = "maybe perhaps probably possibly might could seems likely guess think suppose unsure quizas quizás tal_vez talvez probablemente posiblemente creo parece supongo puede igual";
const CAUSAL: &str = "because since so therefore thus hence as why porque pues entonces por_eso ya_que debido así_que dado";
const SUBORD: &str = "that which who whom whose when while although though if unless whereas que cual cuales quien cuando mientras aunque si donde cuyo";
const CONJUNCTIONS: &str = "and or also plus but y o e u pero además ademas también tambien";
const CONDITIONALS: &str = "if unless otherwise else si sino excepto except";
const NEGATIONS: &str = "no not never nunca nada ni tampoco without sin";

#[derive(Clone, Debug)]
struct TrainingText {
    text: String,
    replies_to_question: bool,
}

#[derive(Clone, Debug)]
struct TimedMessage {
    timestamp: DateTime<Utc>,
    role: Role,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    User,
    Assistant,
}

pub(crate) fn calibrate(
    connection: &Connection,
    min_samples: usize,
    localizer: &Localizer,
) -> Result<()> {
    let mut statement = connection.prepare(
        "SELECT m.metric_id,d.key,m.direction,h.value,m.value
         FROM metric_values m
         JOIN metric_values h ON h.turn_id=m.turn_id AND h.metric_id=m.metric_id
              AND h.direction=m.direction AND h.source='heuristic'
         JOIN metric_definitions d ON d.id=m.metric_id
         JOIN metric_definitions p ON p.id=d.parent_id AND p.key='nebulosity'
         WHERE m.source='model'",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
            row.get::<_, f64>(4)?,
        ))
    })?;
    let mut groups: BTreeMap<(i64, String, String), Vec<(f64, f64)>> = BTreeMap::new();
    for row in rows {
        let (metric, key, direction, x, y) = row?;
        groups
            .entry((metric, direction, key))
            .or_default()
            .push((x, y));
    }

    println!("{}", localizer.text("learning.calibrate_header"));
    for ((metric, direction, key), pairs) in groups {
        if pairs.len() < min_samples {
            println!(
                "{key:<22} {direction}  {:<4} {}",
                pairs.len(),
                localizer.format(
                    "learning.calibrate_skip",
                    &[("samples", min_samples.to_string())]
                )
            );
            continue;
        }
        let n = pairs.len() as f64;
        let mean_x = pairs.iter().map(|pair| pair.0).sum::<f64>() / n;
        let mean_y = pairs.iter().map(|pair| pair.1).sum::<f64>() / n;
        let sxx = pairs
            .iter()
            .map(|pair| (pair.0 - mean_x).powi(2))
            .sum::<f64>();
        let syy = pairs
            .iter()
            .map(|pair| (pair.1 - mean_y).powi(2))
            .sum::<f64>();
        let sxy = pairs
            .iter()
            .map(|pair| (pair.0 - mean_x) * (pair.1 - mean_y))
            .sum::<f64>();
        let correlation = if sxx > 0. && syy > 0. {
            sxy / (sxx * syy).sqrt()
        } else {
            0.
        };
        let slope = if sxx > 0. && correlation > 0.1 {
            sxy / sxx
        } else {
            0.
        };
        let intercept = mean_y - slope * mean_x;
        let before = pairs.iter().map(|pair| pair.1 - pair.0).sum::<f64>() / n;
        let after = pairs
            .iter()
            .map(|pair| pair.1 - (slope * pair.0 + intercept).clamp(0., 100.))
            .sum::<f64>()
            / n;
        connection.execute(
            "INSERT OR REPLACE INTO heuristic_calibration
             (metric_id,direction,slope,intercept,correlation,samples,fitted_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                metric,
                direction,
                round(slope, 4),
                round(intercept, 2),
                round(correlation, 3),
                pairs.len(),
                timestamp()
            ],
        )?;
        println!(
            "{key:<22} {direction}  {:<4} {correlation:+.2}  {slope:6.3}  {intercept:9.1}  {before:+6.1} -> {after:+.1}",
            pairs.len()
        );
    }
    Ok(())
}

pub(crate) fn fit(
    connection: &Connection,
    min_samples: usize,
    dry_run: bool,
    localizer: &Localizer,
) -> Result<()> {
    fit_with_roots(
        connection,
        min_samples,
        dry_run,
        localizer,
        &transcript_roots(),
    )
}

fn fit_with_roots(
    connection: &Connection,
    min_samples: usize,
    dry_run: bool,
    localizer: &Localizer,
    roots: &[PathBuf],
) -> Result<()> {
    let texts = training_texts(connection, roots)?;
    let features = texts
        .iter()
        .map(|(key, value)| {
            (
                key.clone(),
                feature_vector(&text_features(&value.text, value.replies_to_question)),
            )
        })
        .collect::<HashMap<_, _>>();
    println!(
        "{}",
        localizer.format("learning.fit_texts", &[("count", texts.len().to_string())])
    );
    println!("{}", localizer.text("learning.fit_header"));

    let mut adopted = Vec::new();
    for direction in ["u2a", "a2u"] {
        for key in KEYS {
            let mut query = connection.prepare(
                "SELECT v.turn_id,v.value FROM metric_values v
                 JOIN metric_definitions d ON d.id=v.metric_id
                 WHERE d.key=?1 AND v.direction=?2 AND v.source='model'",
            )?;
            let rows = query.query_map(params![key, direction], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, f64>(1)?))
            })?;
            let mut data = Vec::new();
            for row in rows {
                let (turn, target) = row?;
                let lookup = (turn, direction.to_owned());
                if let (Some(vector), Some(text)) = (features.get(&lookup), texts.get(&lookup)) {
                    data.push((vector.clone(), target, text.clone()));
                }
            }
            if data.len() < min_samples || data.len() < FOLDS + 1 {
                continue;
            }
            let xs = data.iter().map(|item| item.0.clone()).collect::<Vec<_>>();
            let ys = data.iter().map(|item| item.1).collect::<Vec<_>>();
            let rule_predictions = data
                .iter()
                .map(|item| {
                    heuristic(&item.2.text, item.2.replies_to_question)
                        .get(key)
                        .copied()
                        .unwrap_or(0.)
                })
                .collect::<Vec<_>>();
            let rule_correlation = pearson(&rule_predictions, &ys);
            let (cv_correlation, penalty) = RIDGE_PENALTIES
                .into_iter()
                .map(|penalty| (cv_score(&xs, &ys, penalty, FOLDS), penalty))
                .max_by(|left, right| left.0.total_cmp(&right.0))
                .expect("ridge penalties are non-empty");
            let take = cv_correlation >= 0.2 && cv_correlation > rule_correlation.max(0.) + 0.05;
            println!(
                "{key:<22} {direction}  {:<4} {rule_correlation:+.2}   {cv_correlation:+.2}         {penalty:<6}  {}",
                data.len(),
                localizer.text(if take { "learning.yes" } else { "learning.no" })
            );
            if take && !dry_run {
                let (weights, intercept) = ridge_fit(&xs, &ys, penalty);
                let metric = metric_id(connection, key)?;
                connection.execute(
                    "DELETE FROM heuristic_weights WHERE metric_id=?1 AND direction=?2",
                    params![metric, direction],
                )?;
                for (feature, weight) in feature_names().iter().zip(weights) {
                    connection.execute(
                        "INSERT INTO heuristic_weights(metric_id,direction,feature,weight)
                         VALUES(?1,?2,?3,?4)",
                        params![metric, direction, feature, weight],
                    )?;
                }
                connection.execute(
                    "INSERT INTO heuristic_weights(metric_id,direction,feature,weight)
                     VALUES(?1,?2,'__intercept__',?3)",
                    params![metric, direction, intercept],
                )?;
                adopted.push((key, direction));
            }
        }
    }
    if dry_run {
        return Ok(());
    }

    for ((turn, direction), text) in &texts {
        for (key, value) in
            learned_heuristic(connection, &text.text, text.replies_to_question, direction)?
        {
            put_metric(connection, *turn, &key, value, "heuristic", Some(direction))?;
        }
    }
    connection.execute("DELETE FROM heuristic_calibration", [])?;
    println!(
        "{}",
        localizer.format("learning.fit_done", &[("count", adopted.len().to_string())])
    );
    Ok(())
}

pub(crate) fn learned_heuristic(
    connection: &Connection,
    text: &str,
    replies_to_question: bool,
    direction: &str,
) -> Result<BTreeMap<String, f64>> {
    let mut output = heuristic(text, replies_to_question);
    if !WORD.is_match(text) {
        return Ok(output);
    }
    let features = text_features(text, replies_to_question);
    let mut statement = connection.prepare(
        "SELECT d.key,w.feature,w.weight FROM heuristic_weights w
         JOIN metric_definitions d ON d.id=w.metric_id WHERE w.direction=?1",
    )?;
    let rows = statement.query_map([direction], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, f64>(2)?,
        ))
    })?;
    let mut models: HashMap<String, HashMap<String, f64>> = HashMap::new();
    for row in rows {
        let (metric, feature, weight) = row?;
        models.entry(metric).or_default().insert(feature, weight);
    }
    for key in KEYS {
        if let Some(weights) = models.get(key) {
            let value = weights.get("__intercept__").copied().unwrap_or(0.)
                + weights
                    .iter()
                    .filter(|(name, _)| name.as_str() != "__intercept__")
                    .map(|(name, weight)| features.get(name).copied().unwrap_or(0.) * weight)
                    .sum::<f64>();
            output.insert(key.to_owned(), round(value.clamp(0., 100.), 1));
        }
    }
    if replies_to_question {
        output.insert("context_gap".into(), 0.);
    }
    Ok(output)
}

fn text_features(text: &str, replies_to_question: bool) -> BTreeMap<String, f64> {
    let words = WORD
        .find_iter(text)
        .map(|word| word.as_str())
        .collect::<Vec<_>>();
    let lower = words
        .iter()
        .map(|word| word.to_lowercase())
        .collect::<Vec<_>>();
    let sentences = text
        .split(|character| matches!(character, '.' | '!' | '?' | '¿' | '¡' | '\n'))
        .filter(|sentence| WORD.is_match(sentence))
        .collect::<Vec<_>>();
    let sentence_count = sentences.len().max(1) as f64;
    let word_count = words.len() as f64;
    let divisor = word_count.max(1.);
    let sentence_lengths = sentences
        .iter()
        .map(|sentence| WORD.find_iter(sentence).count() as f64)
        .collect::<Vec<_>>();
    let lines = text.lines().collect::<Vec<_>>();
    let rules = heuristic(text, replies_to_question);
    let mut features = BTreeMap::from([
        ("log_words".into(), word_count.ln_1p()),
        ("log_sentences".into(), sentence_count.ln_1p()),
        ("avg_sentence_len".into(), word_count / sentence_count),
        (
            "max_sentence_len".into(),
            sentence_lengths.into_iter().fold(0., f64::max),
        ),
        ("question_marks".into(), text.matches('?').count() as f64),
        ("unmarked_questions".into(), unmarked_questions(text) as f64),
        ("vague_ratio".into(), ratio_in(&lower, VAGUE, divisor)),
        ("hedge_ratio".into(), ratio_in(&lower, HEDGES, divisor)),
        ("causal_ratio".into(), ratio_in(&lower, CAUSAL, divisor)),
        (
            "subord_per_sentence".into(),
            count_in(&lower, SUBORD) / sentence_count,
        ),
        (
            "conj_per_sentence".into(),
            count_in(&lower, CONJUNCTIONS) / sentence_count,
        ),
        (
            "conditionals".into(),
            ratio_in(&lower, CONDITIONALS, divisor),
        ),
        ("negations".into(), ratio_in(&lower, NEGATIONS, divisor)),
        (
            "commas_per_sentence".into(),
            text.matches(',').count() as f64 / sentence_count,
        ),
        (
            "anchors_per_100w".into(),
            CONCRETE.find_iter(text).count() as f64 * 100. / divisor,
        ),
        (
            "code_spans".into(),
            (text.matches('`').count() / 2 + text.matches("```").count()) as f64,
        ),
        (
            "list_lines".into(),
            lines.iter().filter(|line| LIST_LINE.is_match(line)).count() as f64,
        ),
        (
            "headings".into(),
            lines
                .iter()
                .filter(|line| line.trim_start().starts_with('#'))
                .count() as f64,
        ),
        (
            "table_rows".into(),
            lines
                .iter()
                .filter(|line| line.trim_start().starts_with('|'))
                .count() as f64,
        ),
        (
            "long_word_ratio".into(),
            words
                .iter()
                .filter(|word| word.chars().count() > 12)
                .count() as f64
                / divisor,
        ),
        (
            "type_token".into(),
            lower.iter().collect::<HashSet<_>>().len() as f64 / divisor,
        ),
        (
            "imperatives".into(),
            IMPERATIVE.find_iter(text).count() as f64,
        ),
        (
            "no_punctuation".into(),
            if word_count > 6. && !text.contains(['.', '!', '?', ';', ':']) {
                1.
            } else {
                0.
            },
        ),
        (
            "replies_to_question".into(),
            if replies_to_question { 1. } else { 0. },
        ),
    ]);
    for (key, value) in rules {
        features.insert(format!("rule_{key}"), value);
    }
    features
}

fn feature_names() -> Vec<String> {
    text_features("", false).into_keys().collect()
}

fn feature_vector(features: &BTreeMap<String, f64>) -> Vec<f64> {
    feature_names()
        .iter()
        .map(|name| features.get(name).copied().unwrap_or(0.))
        .collect()
}

fn ratio_in(words: &[String], choices: &str, divisor: f64) -> f64 {
    count_in(words, choices) / divisor
}

fn count_in(words: &[String], choices: &str) -> f64 {
    let choices = choices.split_whitespace().collect::<HashSet<_>>();
    words
        .iter()
        .filter(|word| choices.contains(word.as_str()))
        .count() as f64
}

fn unmarked_questions(text: &str) -> usize {
    text.split(|character| matches!(character, '.' | '!' | '?' | '\n'))
        .filter(|sentence| {
            let sentence = sentence.trim();
            let words = WORD.find_iter(sentence).collect::<Vec<_>>();
            !words.is_empty()
                && !sentence.contains('?')
                && !sentence.starts_with(['`', '-', '*', '|', '#'])
                && ((words.len() > 2
                    && QUESTION_START.contains(words[0].as_str().to_lowercase().as_str()))
                    || QUESTION_PHRASE.is_match(sentence))
        })
        .count()
}

fn solve(matrix: &[Vec<f64>], values: &[f64]) -> Vec<f64> {
    let n = matrix.len();
    let mut augmented = matrix
        .iter()
        .zip(values)
        .map(|(row, value)| {
            let mut row = row.clone();
            row.push(*value);
            row
        })
        .collect::<Vec<_>>();
    for column in 0..n {
        let pivot = (column..n)
            .max_by(|left, right| {
                augmented[*left][column]
                    .abs()
                    .total_cmp(&augmented[*right][column].abs())
            })
            .unwrap();
        augmented.swap(column, pivot);
        if augmented[column][column].abs() < 1e-12 {
            continue;
        }
        for row in 0..n {
            if row == column {
                continue;
            }
            let factor = augmented[row][column] / augmented[column][column];
            for entry in column..=n {
                augmented[row][entry] -= factor * augmented[column][entry];
            }
        }
    }
    (0..n)
        .map(|index| {
            if augmented[index][index].abs() > 1e-12 {
                augmented[index][n] / augmented[index][index]
            } else {
                0.
            }
        })
        .collect()
}

fn ridge_fit(xs: &[Vec<f64>], ys: &[f64], penalty: f64) -> (Vec<f64>, f64) {
    let dimensions = xs[0].len();
    let means = (0..dimensions)
        .map(|column| xs.iter().map(|row| row[column]).sum::<f64>() / xs.len() as f64)
        .collect::<Vec<_>>();
    let deviations = (0..dimensions)
        .map(|column| {
            (xs.iter()
                .map(|row| (row[column] - means[column]).powi(2))
                .sum::<f64>()
                / xs.len() as f64)
                .sqrt()
                .max(1e-9)
        })
        .collect::<Vec<_>>();
    let standardized = xs
        .iter()
        .map(|row| {
            (0..dimensions)
                .map(|column| (row[column] - means[column]) / deviations[column])
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let target_mean = ys.iter().sum::<f64>() / ys.len() as f64;
    let matrix = (0..dimensions)
        .map(|left| {
            (0..dimensions)
                .map(|right| {
                    standardized
                        .iter()
                        .map(|row| row[left] * row[right])
                        .sum::<f64>()
                        + if left == right { penalty } else { 0. }
                })
                .collect::<Vec<_>>()
        })
        .collect::<Vec<_>>();
    let values = (0..dimensions)
        .map(|column| {
            standardized
                .iter()
                .zip(ys)
                .map(|(row, target)| row[column] * (target - target_mean))
                .sum::<f64>()
        })
        .collect::<Vec<_>>();
    let standardized_weights = solve(&matrix, &values);
    let weights = standardized_weights
        .iter()
        .zip(&deviations)
        .map(|(weight, deviation)| weight / deviation)
        .collect::<Vec<_>>();
    let intercept = target_mean
        - weights
            .iter()
            .zip(means)
            .map(|(weight, mean)| weight * mean)
            .sum::<f64>();
    (weights, intercept)
}

fn predict(weights: &[f64], intercept: f64, values: &[f64]) -> f64 {
    (intercept
        + weights
            .iter()
            .zip(values)
            .map(|(weight, value)| weight * value)
            .sum::<f64>())
    .clamp(0., 100.)
}

fn pearson(left: &[f64], right: &[f64]) -> f64 {
    if left.is_empty() || left.len() != right.len() {
        return 0.;
    }
    let left_mean = left.iter().sum::<f64>() / left.len() as f64;
    let right_mean = right.iter().sum::<f64>() / right.len() as f64;
    let covariance = left
        .iter()
        .zip(right)
        .map(|(left, right)| (left - left_mean) * (right - right_mean))
        .sum::<f64>();
    let left_variance = left
        .iter()
        .map(|value| (value - left_mean).powi(2))
        .sum::<f64>();
    let right_variance = right
        .iter()
        .map(|value| (value - right_mean).powi(2))
        .sum::<f64>();
    if left_variance > 0. && right_variance > 0. {
        covariance / (left_variance * right_variance).sqrt()
    } else {
        0.
    }
}

fn cv_score(xs: &[Vec<f64>], ys: &[f64], penalty: f64, folds: usize) -> f64 {
    let mut predictions = vec![0.; xs.len()];
    for fold in 0..folds {
        let training = (0..xs.len())
            .filter(|index| index % folds != fold)
            .collect::<Vec<_>>();
        if training.is_empty() {
            return 0.;
        }
        let train_x = training
            .iter()
            .map(|index| xs[*index].clone())
            .collect::<Vec<_>>();
        let train_y = training.iter().map(|index| ys[*index]).collect::<Vec<_>>();
        let (weights, intercept) = ridge_fit(&train_x, &train_y, penalty);
        for index in (0..xs.len()).filter(|index| index % folds == fold) {
            predictions[index] = predict(&weights, intercept, &xs[index]);
        }
    }
    pearson(&predictions, ys)
}

fn transcript_roots() -> Vec<PathBuf> {
    if let Some(value) = env::var_os("NEBULAR_TRANSCRIPTS") {
        return env::split_paths(&value).collect();
    }
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    vec![
        env::var_os("CLAUDE_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".claude"))
            .join("projects"),
        env::var_os("CODEX_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".codex"))
            .join("sessions"),
    ]
}

fn training_texts(
    connection: &Connection,
    roots: &[PathBuf],
) -> Result<HashMap<(i64, String), TrainingText>> {
    let mut sessions = connection.prepare("SELECT id FROM sessions")?;
    let ids = sessions
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut output = HashMap::new();
    for session in ids {
        let Some(path) = find_transcript(roots, &session) else {
            continue;
        };
        let messages = read_messages(&path);
        let prompts = messages
            .iter()
            .enumerate()
            .filter(|(_, message)| message.role == Role::User && !message.text.trim().is_empty())
            .collect::<Vec<_>>();
        let mut reconstructed = Vec::new();
        let mut previous_answer = String::new();
        for (position, (index, prompt)) in prompts.iter().enumerate() {
            let next = prompts
                .get(position + 1)
                .map(|(index, _)| *index)
                .unwrap_or(messages.len());
            let answer = messages[*index + 1..next]
                .iter()
                .filter(|message| {
                    message.role == Role::Assistant && !message.text.trim().is_empty()
                })
                .next_back()
                .map(|message| message.text.clone())
                .unwrap_or_default();
            reconstructed.push((
                prompt.timestamp,
                prompt.text.clone(),
                answer.clone(),
                previous_answer.contains('?'),
            ));
            previous_answer = answer;
        }
        let mut turns =
            connection.prepare("SELECT id,started_at FROM turns WHERE session_id=?1")?;
        let turns = turns
            .query_map([&session], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (turn, started_at) in turns {
            let Ok(started_at) = DateTime::parse_from_rfc3339(&started_at) else {
                continue;
            };
            let nearest = reconstructed.iter().min_by_key(|item| {
                (item.0.timestamp_millis() - started_at.timestamp_millis()).abs()
            });
            if let Some((stamp, prompt, answer, replies)) = nearest {
                if (stamp.timestamp_millis() - started_at.timestamp_millis()).abs() > 5_000 {
                    continue;
                }
                output.insert(
                    (turn, "u2a".into()),
                    TrainingText {
                        text: prompt.clone(),
                        replies_to_question: *replies,
                    },
                );
                if !answer.is_empty() {
                    output.insert(
                        (turn, "a2u".into()),
                        TrainingText {
                            text: answer.clone(),
                            replies_to_question: prompt.contains('?'),
                        },
                    );
                }
            }
        }
    }
    Ok(output)
}

fn find_transcript(roots: &[PathBuf], session: &str) -> Option<PathBuf> {
    let mut stack = roots.to_vec();
    while let Some(path) = stack.pop() {
        let Ok(metadata) = fs::metadata(&path) else {
            continue;
        };
        if metadata.is_file() {
            let Some(name) = path.file_name() else {
                continue;
            };
            if name.to_string_lossy().contains(session)
                && path
                    .extension()
                    .is_some_and(|extension| extension == "jsonl")
            {
                return Some(path);
            }
            continue;
        }
        if let Ok(entries) = fs::read_dir(path) {
            stack.extend(entries.flatten().map(|entry| entry.path()));
        }
    }
    None
}

fn read_messages(path: &Path) -> Vec<TimedMessage> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    contents
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(message_from_entry)
        .collect()
}

fn message_from_entry(entry: Value) -> Option<TimedMessage> {
    if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
        return None;
    }
    let timestamp = entry
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(|timestamp| DateTime::parse_from_rfc3339(timestamp).ok())?
        .with_timezone(&Utc);
    let payload = entry.get("payload").unwrap_or(&entry);
    let message = entry.get("message").unwrap_or(payload);
    let role = match message
        .get("role")
        .and_then(Value::as_str)
        .or_else(|| entry.get("type").and_then(Value::as_str))?
    {
        "user" => Role::User,
        "assistant" => Role::Assistant,
        _ => return None,
    };
    let content = message.get("content")?;
    let text = match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|block| {
                let kind = block.get("type").and_then(Value::as_str)?;
                if matches!(kind, "text" | "input_text" | "output_text") {
                    block.get("text").and_then(Value::as_str)
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n"),
        _ => return None,
    };
    Some(TimedMessage {
        timestamp,
        role,
        text,
    })
}

fn round(value: f64, digits: i32) -> f64 {
    let factor = 10_f64.powi(digits);
    (value * factor).round() / factor
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use crate::{db, i18n::Lang};
    use serde_json::json;

    fn database() -> Connection {
        let connection = Connection::open_in_memory().unwrap();
        db::initialize(&connection).unwrap();
        connection
    }

    fn english() -> Localizer {
        Localizer::load(Lang::En).unwrap()
    }

    #[test]
    fn feature_set_is_stable_and_cross_validation_finds_signal() {
        let features = text_features("Can you create src/main.rs if needed?", false);
        assert_eq!(features.len(), 34);
        assert!(features["question_marks"] > 0.);
        assert!(features["anchors_per_100w"] > 0.);
        assert!(features["conditionals"] > 0.);

        let xs = (0..50)
            .map(|value| vec![value as f64, (value % 3) as f64])
            .collect::<Vec<_>>();
        let ys = (0..50).map(|value| value as f64 * 1.5).collect::<Vec<_>>();
        assert!(cv_score(&xs, &ys, 1., 5) > 0.99);
    }

    #[test]
    fn calibration_fits_positive_signal() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO sessions(id,started_at)VALUES('s','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        for index in 0..5 {
            connection
                .execute(
                    "INSERT INTO turns(session_id,seq,started_at)VALUES('s',?1,'2026-01-01T00:00:00Z')",
                    [index + 1],
                )
                .unwrap();
            let turn = connection.last_insert_rowid();
            put_metric(
                &connection,
                turn,
                "ambiguity",
                index as f64 * 10.,
                "heuristic",
                Some("u2a"),
            )
            .unwrap();
            put_metric(
                &connection,
                turn,
                "ambiguity",
                index as f64 * 20. + 5.,
                "model",
                Some("u2a"),
            )
            .unwrap();
        }
        calibrate(&connection, 5, &english()).unwrap();
        let fitted: (f64, f64, i64) = connection
            .query_row(
                "SELECT slope,intercept,samples FROM heuristic_calibration",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(fitted, (2., 5., 5));
    }

    #[test]
    fn transcript_reconstruction_keeps_text_in_memory() {
        let connection = database();
        connection
            .execute(
                "INSERT INTO sessions(id,started_at)VALUES('session-x','2026-01-01T00:00:00Z')",
                [],
            )
            .unwrap();
        connection.execute("INSERT INTO turns(session_id,seq,started_at)VALUES('session-x',1,'2026-01-01T00:00:00Z')", []).unwrap();
        let root = std::env::temp_dir().join(format!(
            "nebular-learning-{}-{}",
            std::process::id(),
            connection.last_insert_rowid()
        ));
        fs::create_dir_all(&root).unwrap();
        let lines = [
            json!({"timestamp":"2026-01-01T00:00:00Z","type":"user","message":{"role":"user","content":"private prompt?"}}),
            json!({"timestamp":"2026-01-01T00:00:01Z","type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"private response"}]}}),
        ];
        fs::write(
            root.join("session-x.jsonl"),
            lines
                .iter()
                .map(Value::to_string)
                .collect::<Vec<_>>()
                .join("\n"),
        )
        .unwrap();
        let texts = training_texts(&connection, std::slice::from_ref(&root)).unwrap();
        assert_eq!(texts.len(), 2);
        assert!(texts.values().any(|value| value.text == "private prompt?"));
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM metric_values", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn learned_weights_override_rules_and_keep_reply_context() {
        let connection = database();
        let ambiguity = metric_id(&connection, "ambiguity").unwrap();
        connection
            .execute(
                "INSERT INTO heuristic_weights VALUES(?1,'u2a','__intercept__',73)",
                [ambiguity],
            )
            .unwrap();
        let scores = learned_heuristic(&connection, "A concrete request", false, "u2a").unwrap();
        assert_eq!(scores["ambiguity"], 73.);

        let context = metric_id(&connection, "context_gap").unwrap();
        connection
            .execute(
                "INSERT INTO heuristic_weights VALUES(?1,'a2u','__intercept__',88)",
                [context],
            )
            .unwrap();
        let scores = learned_heuristic(&connection, "yes", true, "a2u").unwrap();
        assert_eq!(scores["context_gap"], 0.);
    }

    #[test]
    fn fit_honors_dry_run_then_adopts_and_recomputes() {
        let connection = database();
        let root = std::env::temp_dir().join(format!(
            "nebular-fit-{}-{}",
            std::process::id(),
            metric_id(&connection, "ambiguity").unwrap()
        ));
        fs::create_dir_all(&root).unwrap();
        for index in 1..=15 {
            let session = format!("fit-session-{index}");
            connection
                .execute(
                    "INSERT INTO sessions(id,started_at)VALUES(?1,'2026-01-01T00:00:00Z')",
                    [&session],
                )
                .unwrap();
            connection
                .execute(
                    "INSERT INTO turns(session_id,seq,started_at)VALUES(?1,1,'2026-01-01T00:00:00Z')",
                    [&session],
                )
                .unwrap();
            let turn = connection.last_insert_rowid();
            put_metric(
                &connection,
                turn,
                "ambiguity",
                index as f64 * 5.,
                "model",
                Some("u2a"),
            )
            .unwrap();
            let prompt = "clear ".repeat(index);
            let entry = json!({
                "timestamp":"2026-01-01T00:00:00Z",
                "type":"user",
                "message":{"role":"user","content":prompt}
            });
            fs::write(root.join(format!("{session}.jsonl")), entry.to_string()).unwrap();
        }
        connection
            .execute(
                "INSERT INTO heuristic_calibration VALUES(?1,'u2a',1,0,1,15,'2026-01-01T00:00:00Z')",
                [metric_id(&connection, "ambiguity").unwrap()],
            )
            .unwrap();

        fit_with_roots(
            &connection,
            10,
            true,
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM heuristic_weights", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM heuristic_calibration", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            1
        );

        fit_with_roots(
            &connection,
            10,
            false,
            &english(),
            std::slice::from_ref(&root),
        )
        .unwrap();
        assert!(
            connection
                .query_row("SELECT COUNT(*) FROM heuristic_weights", [], |row| row
                    .get::<_, i64>(0))
                .unwrap()
                > 0
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM heuristic_calibration", [], |row| row
                    .get::<_, i64>(
                    0
                ))
                .unwrap(),
            0
        );
        assert!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM metric_values WHERE source='heuristic' AND direction='u2a'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap()
                > 0
        );
        fs::remove_dir_all(root).unwrap();
    }
}
