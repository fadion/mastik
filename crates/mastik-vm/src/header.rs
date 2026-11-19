//! A script's header: its title, its description and the inputs Mastik draws as a
//! form before a run. The header is the run of `---` lines the script opens with.
//!
//! ```text
//! --- Card grid
//! --- Lays the selection out as a grid of cards.
//! ---@input cols integer = 3 -- Columns {1..12}
//! ---@input order choice(rows, columns) = rows -- Fill by
//! ```

use std::ops::Range;

use serde_json::{Value, json};

use crate::{ErrorKind, VmError};

/// What a script's header declares.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Header {
    /// The first `---` line.
    pub title: Option<String>,
    /// The `---` lines after the title, one per line; an empty `---` is an empty line.
    pub description: String,
    pub inputs: Vec<Input>,
}

/// One `---@input name type = default -- Label {lo..hi}` line.
#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub name: String,
    pub ty: InputType,
    /// The value the form starts at: the one written, else the type's zero (the
    /// range's low end, the first choice).
    pub default: Value,
    /// The form's label for it; the name when none is written.
    pub label: Option<String>,
    /// The bounds a number or integer is held to.
    pub range: Option<(f64, f64)>,
    /// Its line in the script, from 1.
    pub line: u32,
}

/// The type of a header input.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InputType {
    Number,
    Integer,
    Boolean,
    String,
    /// `#rrggbb` or `#rrggbbaa`, given to the script as that string.
    Color,
    /// One of these words.
    Choice(Vec<String>),
}

impl InputType {
    /// The type as the header spells it.
    pub fn spelled(&self) -> String {
        match self {
            InputType::Number => "number".into(),
            InputType::Integer => "integer".into(),
            InputType::Boolean => "boolean".into(),
            InputType::String => "string".into(),
            InputType::Color => "color".into(),
            InputType::Choice(words) => format!("choice({})", words.join(", ")),
        }
    }
}

/// An error on header line `line`, over `span` of the source.
fn bad(line: u32, span: Range<usize>, message: impl Into<String>) -> VmError {
    VmError {
        kind: ErrorKind::Error,
        message: message.into(),
        line: Some(line),
        span: Some(span),
        fix: None,
    }
}

/// Read the header `source` opens with; a line of it that does not parse is an
/// error on that line.
pub fn header(source: &str) -> Result<Header, VmError> {
    let mut out = Header::default();
    let mut offset = 0;
    let mut description: Vec<String> = Vec::new();
    for (i, raw) in source.split_inclusive('\n').enumerate() {
        let line_no = u32::try_from(i + 1).unwrap_or(u32::MAX);
        let start = offset;
        offset += raw.len();
        let text = raw.trim_end_matches(['\n', '\r']);
        let lead = text.len() - text.trim_start().len();
        let Some(body) = text.trim_start().strip_prefix("---") else {
            if text.trim().is_empty() && out.title.is_none() {
                continue;
            }
            break;
        };
        let body_at = start + lead + 3;
        if let Some(tagged) = body.strip_prefix('@') {
            let word_end = tagged
                .find(|c: char| !c.is_ascii_alphanumeric() && c != '_')
                .unwrap_or(tagged.len());
            let tag = &tagged[..word_end];
            if tag != "input" {
                return Err(bad(
                    line_no,
                    body_at..body_at + 1 + word_end,
                    format!("No header tag “@{tag}” — the header takes `---@input`"),
                ));
            }
            let at = body_at + 1 + word_end;
            let input = input(&tagged[word_end..], at, line_no)?;
            if out.inputs.iter().any(|x| x.name == input.name) {
                return Err(bad(
                    line_no,
                    start..start + text.len(),
                    format!("Two inputs are called “{}”", input.name),
                ));
            }
            out.inputs.push(input);
            continue;
        }
        let words = body.trim();
        if out.title.is_none() && out.inputs.is_empty() && description.is_empty() {
            if !words.is_empty() {
                out.title = Some(words.to_owned());
            }
        } else {
            description.push(words.to_owned());
        }
    }
    while description.last().is_some_and(String::is_empty) {
        description.pop();
    }
    out.description = description.join("\n");
    Ok(out)
}

/// The byte past the closing quote of an input's quoted default, which may hold
/// ` --`; 0 when the default before the first ` --` is not quoted or not closed.
/// A default has no escapes (`parse_default` only strips its quotes), so the
/// next matching quote closes it, even after a `\`.
fn quoted_end(text: &str) -> usize {
    let dash = text.find(" --").unwrap_or(text.len());
    let Some(eq) = text[..dash].find('=') else {
        return 0;
    };
    let value = text[eq + 1..].trim_start();
    let open = text.len() - value.len();
    let Some(quote) = value.bytes().next().filter(|q| matches!(q, b'"' | b'\'')) else {
        return 0;
    };
    text.as_bytes()[open + 1..]
        .iter()
        .position(|&c| c == quote)
        .map_or(0, |p| open + 1 + p + 1)
}

/// One input from the text after `@input`, which starts at byte `at` of the source.
fn input(text: &str, at: usize, line: u32) -> Result<Input, VmError> {
    let whole = at..at + text.len();
    // The label is what follows the first ` --` past a quoted default.
    let from = quoted_end(text);
    let (decl, label) = match text[from..].find(" --") {
        Some(p) => (&text[..from + p], Some(&text[from + p + 3..])),
        None => (text, None),
    };
    let (label, range_text) = match label {
        Some(l) => {
            let l = l.trim();
            match (l.rfind('{'), l.ends_with('}')) {
                (Some(open), true) => (l[..open].trim(), Some(&l[open..])),
                _ => (l, None),
            }
        }
        None => ("", None),
    };
    let lead = decl.len() - decl.trim_start().len();
    let decl_trimmed = decl.trim_start();
    let name_len = decl_trimmed
        .find(|c: char| !crate::lex::ident_char(c))
        .unwrap_or(decl_trimmed.len());
    let name = &decl_trimmed[..name_len];
    let name_at = at + lead;
    if name.is_empty() || !name.starts_with(crate::lex::ident_start) {
        return Err(bad(
            line,
            whole,
            "An input reads `---@input name type = default -- Label`",
        ));
    }
    let after_name = &decl_trimmed[name_len..];
    let ty_lead = after_name.len() - after_name.trim_start().len();
    let ty_text = after_name.trim_start();
    let ty_at = name_at + name_len + ty_lead;
    let ty_len = if ty_text.starts_with("choice(") {
        ty_text.find(')').map(|p| p + 1).ok_or_else(|| {
            bad(
                line,
                ty_at..ty_at + ty_text.len(),
                "`choice(` needs its closing `)`",
            )
        })?
    } else {
        ty_text
            .find(|c: char| c.is_whitespace() || c == '=')
            .unwrap_or(ty_text.len())
    };
    let ty_word = &ty_text[..ty_len];
    let ty_span = ty_at..ty_at + ty_len;
    let ty = match ty_word {
        "number" => InputType::Number,
        "integer" => InputType::Integer,
        "boolean" => InputType::Boolean,
        "string" => InputType::String,
        "color" => InputType::Color,
        w if w.starts_with("choice(") => {
            let words: Vec<String> = w["choice(".len()..w.len() - 1]
                .split(',')
                .map(|s| s.trim().to_owned())
                .collect();
            if words.iter().any(String::is_empty) {
                return Err(bad(
                    line,
                    ty_span,
                    "A choice lists its words: `choice(a, b)`",
                ));
            }
            InputType::Choice(words)
        }
        "" => {
            return Err(bad(
                line,
                whole,
                format!(
                    "“{name}” needs a type: number, integer, boolean, string, color or choice(…)"
                ),
            ));
        }
        other => {
            return Err(bad(
                line,
                ty_span,
                format!(
                    "No input type “{other}” — the types are number, integer, boolean, string, color and choice(…)"
                ),
            ));
        }
    };
    let rest = &ty_text[ty_len..];
    let rest_at = ty_at + ty_len;
    let default_text = match rest.trim_start().strip_prefix('=') {
        Some(d) => Some(d.trim()),
        None if rest.trim().is_empty() => None,
        None => {
            return Err(bad(
                line,
                rest_at..rest_at + rest.len(),
                "A default follows `=`: `---@input gap number = 24`",
            ));
        }
    };
    let default_span = default_text.map_or(whole.clone(), |d| {
        let p = rest.rfind(d).unwrap_or(0);
        rest_at + p..rest_at + p + d.len()
    });
    let range = match range_text {
        None => None,
        Some(r) => {
            let inner = &r[1..r.len() - 1];
            let parsed = inner.split_once("..").and_then(|(lo, hi)| {
                Some((
                    lo.trim().parse::<f64>().ok()?,
                    hi.trim().parse::<f64>().ok()?,
                ))
            });
            let Some((lo, hi)) =
                parsed.filter(|(lo, hi)| lo.is_finite() && hi.is_finite() && lo <= hi)
            else {
                return Err(bad(line, whole, "A range reads `{lo..hi}`, low first"));
            };
            if !matches!(ty, InputType::Number | InputType::Integer) {
                return Err(bad(
                    line,
                    whole,
                    "Only a number or an integer takes a range",
                ));
            }
            Some((lo, hi))
        }
    };
    let default = match default_text {
        Some(d) => parse_default(&ty, d).map_err(|why| bad(line, default_span.clone(), why))?,
        None => zero(&ty, range),
    };
    if let (Some((lo, hi)), Some(v)) = (range, default.as_f64())
        && !(lo..=hi).contains(&v)
    {
        return Err(bad(
            line,
            default_span,
            format!("The default {v} is outside {lo}..{hi}"),
        ));
    }
    Ok(Input {
        name: name.to_owned(),
        ty,
        default,
        label: (!label.is_empty()).then(|| label.to_owned()),
        range,
        line,
    })
}

/// The value an input of `ty` starts at when the header gives none.
fn zero(ty: &InputType, range: Option<(f64, f64)>) -> Value {
    match ty {
        InputType::Number => json!(range.map_or(0.0, |r| r.0)),
        InputType::Integer => json!(range.map_or(0.0, |r| r.0.ceil()) as i64),
        InputType::Boolean => json!(false),
        InputType::String => json!(""),
        InputType::Color => json!("#000000"),
        InputType::Choice(words) => json!(words[0]),
    }
}

/// A default written as `text`, read as `ty`.
fn parse_default(ty: &InputType, text: &str) -> Result<Value, String> {
    match ty {
        InputType::Number => text
            .parse::<f64>()
            .ok()
            .filter(|v| v.is_finite())
            .map(|v| json!(v))
            .ok_or_else(|| format!("“{text}” is not a number")),
        InputType::Integer => text
            .parse::<i64>()
            .map(|v| json!(v))
            .map_err(|_| format!("“{text}” is not a whole number")),
        InputType::Boolean => match text {
            "true" => Ok(json!(true)),
            "false" => Ok(json!(false)),
            _ => Err(format!("“{text}” is not true or false")),
        },
        InputType::String => {
            let quoted = text.len() >= 2
                && ((text.starts_with('"') && text.ends_with('"'))
                    || (text.starts_with('\'') && text.ends_with('\'')));
            Ok(json!(if quoted {
                &text[1..text.len() - 1]
            } else {
                text
            }))
        }
        InputType::Color => {
            let hex = text.strip_prefix('#').unwrap_or("");
            if matches!(hex.len(), 6 | 8) && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                Ok(json!(format!("#{}", hex.to_ascii_lowercase())))
            } else {
                Err(format!(
                    "“{text}” is not a colour: write #rrggbb or #rrggbbaa"
                ))
            }
        }
        InputType::Choice(words) => words
            .iter()
            .find(|w| *w == text)
            .map(|w| json!(w))
            .ok_or_else(|| format!("“{text}” is not one of {}", words.join(", "))),
    }
}

impl Header {
    /// What a run reads as `param`: each input's value from `given`, read as its
    /// default is written (a JSON value as its text) and held to its range, else
    /// its default. A name the header does not declare is refused.
    pub fn params(
        &self,
        given: &serde_json::Map<String, Value>,
    ) -> Result<serde_json::Map<String, Value>, String> {
        if let Some(stray) = given
            .keys()
            .find(|k| !self.inputs.iter().any(|i| &i.name == *k))
        {
            let names: Vec<&str> = self.inputs.iter().map(|i| i.name.as_str()).collect();
            return Err(match names.as_slice() {
                [] => format!("the script declares no inputs, so no `{stray}`"),
                _ => format!("no input `{stray}`; the script's are {}", names.join(", ")),
            });
        }
        let mut out = serde_json::Map::new();
        for input in &self.inputs {
            let value = match given.get(&input.name) {
                None => input.default.clone(),
                Some(v) => {
                    let text = match v {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    };
                    let value = parse_default(&input.ty, &text)
                        .map_err(|why| format!("`{}`: {why}", input.name))?;
                    if let (Some((lo, hi)), Some(n)) = (input.range, value.as_f64())
                        && !(lo..=hi).contains(&n)
                    {
                        return Err(format!("`{}`: {n} is outside {lo}..{hi}", input.name));
                    }
                    value
                }
            };
            out.insert(input.name.clone(), value);
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A given value is read by its input's type and range; the rest keep
    /// their defaults, and an undeclared name is refused.
    #[test]
    fn params_read_given_values_by_type_and_fill_defaults() {
        let h = header(
            "---@input cols integer = 3 -- Columns {1..12}\n\
             ---@input brand string = acme\n\
             ---@input tint color = #ff0000\n",
        )
        .unwrap();
        let given = |pairs: &[(&str, Value)]| -> serde_json::Map<String, Value> {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect()
        };
        let p = h.params(&given(&[("cols", json!("5"))])).unwrap();
        assert_eq!(p["cols"], json!(5));
        assert_eq!(p["brand"], json!("acme"), "a default");
        assert_eq!(
            h.params(&given(&[("cols", json!(4))])).unwrap()["cols"],
            json!(4)
        );
        assert!(
            h.params(&given(&[("cols", json!("20"))])).is_err(),
            "out of range"
        );
        assert!(h.params(&given(&[("cols", json!("two"))])).is_err());
        assert!(h.params(&given(&[("tint", json!("red"))])).is_err());
        assert!(
            h.params(&given(&[("logo", json!("x"))])).is_err(),
            "undeclared"
        );
    }
}
