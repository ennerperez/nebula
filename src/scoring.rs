use crate::i18n::Localizer;
use anyhow::{Result, anyhow};
use once_cell::sync::Lazy;
use regex::Regex;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const KEYS: [&str; 10] = [
    "ambiguity",
    "idea_branching",
    "idea_dispersion",
    "context_gap",
    "explanation_gap",
    "argument_gap",
    "lucidity_deficit",
    "structural_complexity",
    "opacity",
    "certainty_deficit",
];
static WORD: Lazy<Regex> = Lazy::new(|| Regex::new(r"[\p{L}0-9_']+").unwrap());
static CONCRETE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(`[^`]+`|/[\w.-]+/|\w+\.\w{1,5}\b|\b[A-Z]{2,}-\d+\b|\d+|https?://)").unwrap()
});
static LIST_LINE: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*([-*+]|\d+[.)])\s").unwrap());
static CORRECTION: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)^\s*(no[,.! ]|nope|wrong|that's not|thats not|not what|again|i said|i told you|eso no|no es|otra vez|de nuevo|te dije|ya te dije|est[aá] mal|incorrecto|no entend)").unwrap()
});
static QUESTION_PHRASE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(me entiendes|me explico|tiene sentido|makes? sense)\b|\b(verdad|cierto|right|correct)\s*$").unwrap()
});

const VAGUE: &[&str] = &[
    "thing",
    "things",
    "stuff",
    "something",
    "somehow",
    "somewhere",
    "whatever",
    "etc",
    "it",
    "that",
    "this",
    "those",
    "these",
    "algo",
    "cosa",
    "cosas",
    "eso",
    "esto",
    "esa",
    "ese",
    "aquello",
    "lo",
    "la",
    "le",
    "así",
    "tal",
];
const HEDGES: &[&str] = &[
    "maybe",
    "perhaps",
    "probably",
    "possibly",
    "might",
    "could",
    "seems",
    "likely",
    "guess",
    "think",
    "suppose",
    "unsure",
    "quizas",
    "quizás",
    "tal_vez",
    "talvez",
    "probablemente",
    "posiblemente",
    "creo",
    "parece",
    "supongo",
    "puede",
    "igual",
];
const CAUSAL: &[&str] = &[
    "because",
    "since",
    "so",
    "therefore",
    "thus",
    "hence",
    "as",
    "why",
    "porque",
    "pues",
    "entonces",
    "por_eso",
    "ya_que",
    "debido",
    "así_que",
    "dado",
];
const SUBORD: &[&str] = &[
    "that", "which", "who", "whom", "whose", "when", "while", "although", "though", "if", "unless",
    "whereas", "que", "cual", "cuales", "quien", "cuando", "mientras", "aunque", "si", "donde",
    "cuyo",
];
const STOP: &[&str] = &[
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "is", "are", "be", "was",
    "it", "this", "that", "i", "you", "we", "me", "my", "your", "el", "la", "los", "las", "un",
    "una", "y", "o", "de", "del", "en", "con", "por", "para", "es", "son", "ser", "que", "lo",
    "mi", "tu", "te", "se", "no", "si",
];
const PRONOUN_START: &[&str] = &[
    "it", "this", "that", "they", "eso", "esto", "ese", "esa", "lo",
];
const QUESTION_START: &[&str] = &[
    "que",
    "qué",
    "como",
    "cómo",
    "cual",
    "cuál",
    "cuales",
    "cuáles",
    "donde",
    "dónde",
    "cuando",
    "cuándo",
    "cuanto",
    "cuánto",
    "quien",
    "quién",
    "puedes",
    "podrías",
    "podrias",
    "podemos",
    "deberíamos",
    "deberiamos",
    "es",
    "está",
    "esta",
    "hay",
    "tienes",
    "sabes",
    "what",
    "how",
    "why",
    "which",
    "where",
    "when",
    "who",
    "whom",
    "whose",
    "should",
    "can",
    "could",
    "would",
    "will",
    "do",
    "does",
    "did",
    "is",
    "are",
    "was",
    "were",
    "have",
    "has",
    "may",
    "shall",
];

pub(crate) fn parse_scores(
    raw: &str,
    name: &str,
    localizer: &Localizer,
) -> Result<BTreeMap<String, f64>> {
    let value: Value = serde_json::from_str(raw).map_err(|error| {
        anyhow!(localizer.format(
            "log.bad_json",
            &[("dir", name.into()), ("err", error.to_string())]
        ))
    })?;
    let missing = KEYS
        .into_iter()
        .filter(|key| value.get(key).and_then(Value::as_f64).is_none())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        return Err(anyhow!(localizer.format(
            "log.missing",
            &[("dir", name.into()), ("keys", missing.join(", "))]
        )));
    }
    KEYS.into_iter()
        .map(|key| {
            Ok((
                key.into(),
                value
                    .get(key)
                    .and_then(Value::as_f64)
                    .expect("validated numeric score")
                    .clamp(0., 100.),
            ))
        })
        .collect()
}

pub(crate) fn word_count(input: &str) -> usize {
    WORD.find_iter(input).count()
}
pub(crate) fn asks_question(input: &str) -> bool {
    input.contains('?')
}
fn words(input: &str) -> Vec<String> {
    WORD.find_iter(input)
        .map(|word| word.as_str().to_lowercase())
        .collect()
}

fn sentences(input: &str) -> Vec<&str> {
    let mut result = Vec::new();
    let mut start = 0;
    for (index, character) in input.char_indices() {
        if matches!(character, '.' | '!' | '?' | '¿' | '¡' | '\n') {
            if WORD.is_match(&input[start..=index]) {
                result.push(&input[start..=index]);
            }
            start = index + character.len_utf8();
        }
    }
    if start < input.len() && WORD.is_match(&input[start..]) {
        result.push(&input[start..]);
    }
    if result.is_empty() && !input.is_empty() {
        result.push(input);
    }
    result
}

fn clamp(value: f64) -> f64 {
    (value.clamp(0., 100.) * 10.).round() / 10.
}

pub(crate) fn heuristic(input: &str, reply: bool) -> BTreeMap<String, f64> {
    let tokens = words(input);
    let n = tokens.len();
    if n == 0 {
        return BTreeMap::new();
    }
    let sentences = sentences(input);
    let ns = sentences.len().max(1) as f64;
    let ratio = |set: &[&str]| {
        tokens
            .iter()
            .filter(|word| set.contains(&word.as_str()))
            .count() as f64
            / n as f64
    };
    let count = |set: &[&str]| {
        tokens
            .iter()
            .filter(|word| set.contains(&word.as_str()))
            .count() as f64
    };
    let vague = ratio(VAGUE);
    let hedge = ratio(HEDGES);
    let causal = count(CAUSAL);
    let list_lines = input
        .lines()
        .filter(|line| LIST_LINE.is_match(line))
        .collect::<Vec<_>>();
    let depth = list_lines
        .iter()
        .map(|line| (line.len() - line.trim_start().len()) / 2 + 1)
        .max()
        .unwrap_or(0);
    let contents = sentences
        .iter()
        .map(|sentence| {
            words(sentence)
                .into_iter()
                .filter(|word| !STOP.contains(&word.as_str()))
                .collect::<BTreeSet<_>>()
        })
        .collect::<Vec<_>>();
    let overlaps = contents
        .iter()
        .enumerate()
        .filter(|(_, current)| !current.is_empty())
        .map(|(index, current)| {
            let others = if contents.len() > 1 {
                contents
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != index)
                    .flat_map(|(_, set)| set.iter().cloned())
                    .collect::<BTreeSet<_>>()
            } else {
                current.clone()
            };
            current.intersection(&others).count() as f64 / current.len() as f64
        })
        .collect::<Vec<_>>();
    let dispersion = 1. - overlaps.iter().sum::<f64>() / overlaps.len().max(1) as f64;
    let pronoun_starts = sentences
        .iter()
        .filter(|sentence| {
            WORD.find(sentence).is_some_and(|first| {
                PRONOUN_START.contains(&first.as_str().to_lowercase().as_str())
            })
        })
        .count() as f64
        / ns;
    BTreeMap::from([
        (
            "ambiguity".into(),
            clamp(vague * 600. + pronoun_starts * 40.),
        ),
        (
            "idea_branching".into(),
            clamp(depth as f64 * 20. + list_lines.len().saturating_sub(3) as f64 * 4.),
        ),
        (
            "idea_dispersion".into(),
            clamp(if ns > 2. { dispersion * 100. } else { 0. }),
        ),
        (
            "context_gap".into(),
            if reply {
                0.
            } else {
                clamp(
                    100. - CONCRETE.find_iter(input).count() as f64 / (n as f64 / 25.).max(1.)
                        * 50.,
                )
            },
        ),
        (
            "explanation_gap".into(),
            if ns > 1. {
                clamp(100. - causal / ns * 150.)
            } else {
                50.
            },
        ),
        (
            "argument_gap".into(),
            clamp((1. - (causal / (ns / 2.).max(1.)).min(1.)) * 70. + hedge * 300.),
        ),
        ("lucidity_deficit".into(), clamp((n as f64 / ns - 12.) * 4.)),
        (
            "structural_complexity".into(),
            clamp(count(SUBORD) / ns * 25. + input.matches(',').count() as f64 / ns * 15.),
        ),
        (
            "opacity".into(),
            clamp(
                tokens
                    .iter()
                    .filter(|word| word.chars().count() > 12)
                    .count() as f64
                    / n as f64
                    * 500.
                    + vague * 200.,
            ),
        ),
        ("certainty_deficit".into(), clamp(hedge * 1000.)),
    ])
}

pub(crate) fn looks_like_correction(input: &str, reply: bool) -> bool {
    !reply && CORRECTION.is_match(input)
}

pub(crate) fn unmarked_questions(input: &str) -> usize {
    sentences(input)
        .into_iter()
        .filter(|sentence| {
            let sentence = sentence.trim();
            let words = words(sentence);
            !words.is_empty()
                && !sentence.contains('?')
                && !sentence.starts_with(['`', '-', '*', '|', '#'])
                && ((QUESTION_START.contains(&words[0].as_str()) && words.len() > 2)
                    || QUESTION_PHRASE.is_match(sentence))
        })
        .count()
}

pub(crate) fn fallback_keywords(input: &str, limit: usize) -> Vec<String> {
    let mut counts = Vec::<(String, usize)>::new();
    for word in words(input).into_iter().filter(|word| {
        word.chars().count() >= 4
            && !STOP.contains(&word.as_str())
            && !VAGUE.contains(&word.as_str())
            && !word.chars().all(|c| c.is_ascii_digit())
    }) {
        if let Some((_, count)) = counts.iter_mut().find(|(seen, _)| seen == &word) {
            *count += 1;
        } else {
            counts.push((word, 1));
        }
    }
    counts.sort_by(|(_, left_count), (_, right_count)| right_count.cmp(left_count));
    counts
        .into_iter()
        .take(limit)
        .map(|(word, _)| word)
        .collect()
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;
    use serde_json::json;
    fn scores(value: f64) -> String {
        Value::Object(
            KEYS.into_iter()
                .map(|key| (key.to_owned(), json!(value)))
                .collect(),
        )
        .to_string()
    }
    #[test]
    fn score_parser_validates_keys_and_clamps_values() {
        let mut input: Value = serde_json::from_str(&scores(50.)).unwrap();
        input["ambiguity"] = json!(-4.);
        input["opacity"] = json!(140.);
        let l = Localizer::load(crate::i18n::Lang::En).unwrap();
        let parsed = parse_scores(&input.to_string(), "u2a", &l).unwrap();
        assert_eq!(parsed["ambiguity"], 0.);
        assert_eq!(parsed["opacity"], 100.);
        assert!(parse_scores("bad", "u2a", &l).is_err());
    }
    #[test]
    fn text_heuristics_cover_questions_corrections_and_keywords() {
        assert!(heuristic("", false).is_empty());
        assert_eq!(heuristic("yes", true)["context_gap"], 0.);
        assert!(looks_like_correction("No, that is wrong", false));
        assert!(!looks_like_correction("no", true));
        assert_eq!(unmarked_questions("Can you fix this now."), 1);
        assert_eq!(
            fallback_keywords("Rust rust tests and code", 2),
            ["rust", "tests"]
        );
    }
}
