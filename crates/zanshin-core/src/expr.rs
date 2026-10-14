//! Human-readable expression language for targeting conditions, round-trippable with [`Condition`].
//!
//! ```text
//! custom.level >= 10 and (default.country in ["DE", "FR"] or computed.session_count > 3)
//!   and not custom.beta == true and computed.last_seen_at within_last 7d
//! ```
//!
//! Precedence: `not` > `and` > `or`; parentheses group. Operators: `== != < <= > >= in not_in
//! contains not_contains starts_with ends_with matches exists not_exists semver_eq semver_gt
//! semver_gte semver_lt semver_lte within_last older_than`. Durations accept `30s 15m 2h 7d 4w`.

use serde_json::Value;

use crate::{
    rules::{Comparison, Condition, Op},
    Error, Result,
};

/// Parse an expression into a condition.
pub fn parse(src: &str) -> Result<Condition> {
    let tokens = lex(src)?;
    let mut p = Parser { tokens, pos: 0 };
    if p.peek().is_none() {
        return Ok(Condition::always());
    }
    let c = p.or()?;
    if let Some(t) = p.peek() {
        return Err(Error::invalid(format!("unexpected `{}`", t.text())));
    }
    c.validate()?;
    Ok(c)
}

/// Render a condition as an expression.
pub fn format(c: &Condition) -> String {
    fmt(c)
}

fn fmt(c: &Condition) -> String {
    let wrap = |s: String, inner_prec: u8, outer_prec: u8| {
        if inner_prec < outer_prec {
            format!("({s})")
        } else {
            s
        }
    };
    match c {
        Condition::All { all } if all.is_empty() => "true".into(),
        Condition::Any { any } if any.is_empty() => "false".into(),
        Condition::All { all } => {
            let parts: Vec<String> = all.iter().map(|x| wrap(fmt(x), prec(x), 2)).collect();
            parts.join(" and ")
        }
        Condition::Any { any } => {
            let parts: Vec<String> = any.iter().map(|x| wrap(fmt(x), prec(x), 1)).collect();
            parts.join(" or ")
        }
        Condition::Not { not } => format!("not {}", wrap(fmt(not), prec(not), 3)),
        Condition::Cmp(cmp) => fmt_cmp(cmp),
    }
}

fn prec(c: &Condition) -> u8 {
    match c {
        Condition::All { all } if all.is_empty() => 4,
        Condition::Any { any } if any.is_empty() => 4,
        Condition::All { .. } => 2,
        Condition::Any { .. } => 1,
        Condition::Not { .. } => 3,
        Condition::Cmp(_) => 4,
    }
}

fn fmt_cmp(c: &Comparison) -> String {
    let op = op_name(c.op);
    match c.op {
        Op::Exists | Op::NotExists => format!("{} {op}", c.prop),
        Op::WithinLast | Op::OlderThan => {
            format!(
                "{} {op} {}",
                c.prop,
                fmt_duration(c.value.as_f64().unwrap_or(0.0))
            )
        }
        _ => format!("{} {op} {}", c.prop, fmt_value(&c.value)),
    }
}

fn fmt_duration(secs: f64) -> String {
    let s = secs as i64;
    if s > 0 && secs.fract() == 0.0 {
        for (unit, n) in [("d", 86400), ("h", 3600), ("m", 60)] {
            if s % n == 0 {
                return format!("{}{unit}", s / n);
            }
        }
    }
    format!("{secs}s")
}

fn fmt_value(v: &Value) -> String {
    match v {
        Value::String(s) => serde_json::to_string(s).unwrap_or_default(),
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(fmt_value).collect::<Vec<_>>().join(", ")
        ),
        other => other.to_string(),
    }
}

fn op_name(op: Op) -> &'static str {
    match op {
        Op::Eq => "==",
        Op::Neq => "!=",
        Op::Gt => ">",
        Op::Gte => ">=",
        Op::Lt => "<",
        Op::Lte => "<=",
        Op::In => "in",
        Op::NotIn => "not_in",
        Op::Contains => "contains",
        Op::NotContains => "not_contains",
        Op::StartsWith => "starts_with",
        Op::EndsWith => "ends_with",
        Op::Matches => "matches",
        Op::Exists => "exists",
        Op::NotExists => "not_exists",
        Op::SemverEq => "semver_eq",
        Op::SemverGt => "semver_gt",
        Op::SemverGte => "semver_gte",
        Op::SemverLt => "semver_lt",
        Op::SemverLte => "semver_lte",
        Op::WithinLast => "within_last",
        Op::OlderThan => "older_than",
    }
}

fn op_from(name: &str) -> Option<Op> {
    Some(match name {
        "==" | "=" | "eq" | "is" => Op::Eq,
        "!=" | "neq" => Op::Neq,
        ">" | "gt" => Op::Gt,
        ">=" | "gte" => Op::Gte,
        "<" | "lt" => Op::Lt,
        "<=" | "lte" => Op::Lte,
        "in" => Op::In,
        "not_in" => Op::NotIn,
        "contains" => Op::Contains,
        "not_contains" => Op::NotContains,
        "starts_with" => Op::StartsWith,
        "ends_with" => Op::EndsWith,
        "matches" => Op::Matches,
        "exists" => Op::Exists,
        "not_exists" => Op::NotExists,
        "semver_eq" => Op::SemverEq,
        "semver_gt" => Op::SemverGt,
        "semver_gte" => Op::SemverGte,
        "semver_lt" => Op::SemverLt,
        "semver_lte" => Op::SemverLte,
        "within_last" => Op::WithinLast,
        "older_than" => Op::OlderThan,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Ident(String),
    Str(String),
    Num(f64),
    Sym(String),
    LParen,
    RParen,
    LBracket,
    RBracket,
    Comma,
}

impl Tok {
    fn text(&self) -> String {
        match self {
            Tok::Ident(s) | Tok::Sym(s) => s.clone(),
            Tok::Str(s) => format!("{s:?}"),
            Tok::Num(n) => n.to_string(),
            Tok::LParen => "(".into(),
            Tok::RParen => ")".into(),
            Tok::LBracket => "[".into(),
            Tok::RBracket => "]".into(),
            Tok::Comma => ",".into(),
        }
    }
}

fn lex(src: &str) -> Result<Vec<Tok>> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            ' ' | '\t' | '\n' | '\r' => i += 1,
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            '[' => {
                out.push(Tok::LBracket);
                i += 1;
            }
            ']' => {
                out.push(Tok::RBracket);
                i += 1;
            }
            ',' => {
                out.push(Tok::Comma);
                i += 1;
            }
            '"' | '\'' => {
                let quote = c;
                let mut s = String::new();
                i += 1;
                loop {
                    let Some(&ch) = chars.get(i) else {
                        return Err(Error::invalid("unterminated string"));
                    };
                    i += 1;
                    match ch {
                        '\\' => {
                            let Some(&e) = chars.get(i) else {
                                return Err(Error::invalid("unterminated string"));
                            };
                            i += 1;
                            s.push(match e {
                                'n' => '\n',
                                't' => '\t',
                                other => other,
                            });
                        }
                        ch if ch == quote => break,
                        ch => s.push(ch),
                    }
                }
                out.push(Tok::Str(s));
            }
            '=' | '!' | '<' | '>' => {
                let mut s = c.to_string();
                if chars.get(i + 1) == Some(&'=') {
                    s.push('=');
                    i += 1;
                }
                i += 1;
                out.push(Tok::Sym(s));
            }
            c if c.is_ascii_digit() || c == '-' => {
                let start = i;
                i += 1;
                while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                    i += 1;
                }
                let text: String = chars[start..i].iter().collect();
                if let Some(secs) = parse_duration(&text) {
                    out.push(Tok::Num(secs));
                } else {
                    let n: f64 = text
                        .parse()
                        .map_err(|_| Error::invalid(format!("bad number `{text}`")))?;
                    out.push(Tok::Num(n));
                }
            }
            c if c.is_alphabetic() || c == '_' || c == '$' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | '$' | '-'))
                {
                    i += 1;
                }
                out.push(Tok::Ident(chars[start..i].iter().collect()));
            }
            other => return Err(Error::invalid(format!("unexpected character `{other}`"))),
        }
    }
    Ok(out)
}

fn parse_duration(text: &str) -> Option<f64> {
    let (num, unit) = text.split_at(text.len() - 1);
    let mult = match unit {
        "s" => 1.0,
        "m" => 60.0,
        "h" => 3600.0,
        "d" => 86400.0,
        "w" => 604800.0,
        _ => return None,
    };
    num.parse::<f64>().ok().map(|n| n * mult)
}

struct Parser {
    tokens: Vec<Tok>,
    pos: usize,
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<Tok> {
        let t = self.tokens.get(self.pos).cloned();
        self.pos += 1;
        t
    }

    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Some(Tok::Ident(s)) if s.eq_ignore_ascii_case(kw))
    }

    fn or(&mut self) -> Result<Condition> {
        let mut items = vec![self.and()?];
        while self.is_kw("or") {
            self.next();
            items.push(self.and()?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Condition::Any { any: items }
        })
    }

    fn and(&mut self) -> Result<Condition> {
        let mut items = vec![self.unary()?];
        while self.is_kw("and") {
            self.next();
            items.push(self.unary()?);
        }
        Ok(if items.len() == 1 {
            items.pop().unwrap()
        } else {
            Condition::All { all: items }
        })
    }

    fn unary(&mut self) -> Result<Condition> {
        if self.is_kw("not") {
            self.next();
            return Ok(Condition::Not {
                not: Box::new(self.unary()?),
            });
        }
        if self.peek() == Some(&Tok::LParen) {
            self.next();
            let c = self.or()?;
            if self.next() != Some(Tok::RParen) {
                return Err(Error::invalid("expected `)`"));
            }
            return Ok(c);
        }
        if self.is_kw("true") {
            self.next();
            return Ok(Condition::All { all: vec![] });
        }
        if self.is_kw("false") {
            self.next();
            return Ok(Condition::Any { any: vec![] });
        }
        self.comparison()
    }

    fn comparison(&mut self) -> Result<Condition> {
        let prop = match self.next() {
            Some(Tok::Ident(p)) => p,
            Some(t) => {
                return Err(Error::invalid(format!(
                    "expected property, got `{}`",
                    t.text()
                )))
            }
            None => return Err(Error::invalid("expected property")),
        };
        let op = match self.next() {
            Some(Tok::Ident(o)) | Some(Tok::Sym(o)) => op_from(&o.to_lowercase())
                .ok_or_else(|| Error::invalid(format!("unknown operator `{o}`")))?,
            Some(t) => {
                return Err(Error::invalid(format!(
                    "expected operator, got `{}`",
                    t.text()
                )))
            }
            None => return Err(Error::invalid(format!("expected operator after `{prop}`"))),
        };
        let value = match op {
            Op::Exists | Op::NotExists => Value::Null,
            _ => self.value()?,
        };
        Ok(Condition::Cmp(Comparison { prop, op, value }))
    }

    fn value(&mut self) -> Result<Value> {
        match self.next() {
            Some(Tok::Str(s)) => Ok(Value::String(s)),
            Some(Tok::Num(n)) => Ok(serde_json::Number::from_f64(n)
                .map(|n| {
                    if n.as_f64()
                        .is_some_and(|f| f.fract() == 0.0 && f.abs() < 9e15)
                    {
                        Value::from(n.as_f64().unwrap() as i64)
                    } else {
                        Value::Number(n)
                    }
                })
                .unwrap_or(Value::Null)),
            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("true") => Ok(Value::Bool(true)),
            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("false") => Ok(Value::Bool(false)),
            Some(Tok::Ident(i)) if i.eq_ignore_ascii_case("null") => Ok(Value::Null),
            Some(Tok::Ident(i)) => Ok(Value::String(i)),
            Some(Tok::LBracket) => {
                let mut items = Vec::new();
                if self.peek() == Some(&Tok::RBracket) {
                    self.next();
                    return Ok(Value::Array(items));
                }
                loop {
                    items.push(self.value()?);
                    match self.next() {
                        Some(Tok::Comma) => continue,
                        Some(Tok::RBracket) => break,
                        _ => return Err(Error::invalid("expected `,` or `]`")),
                    }
                }
                Ok(Value::Array(items))
            }
            Some(t) => Err(Error::invalid(format!("unexpected `{}`", t.text()))),
            None => Err(Error::invalid("expected value")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn round_trip() {
        let src = r#"custom.level >= 10 and (default.country in ["DE", "FR"] or computed.session_count > 3) and not custom.beta == true and computed.last_seen_at within_last 7d"#;
        let c = parse(src).unwrap();
        let json = serde_json::to_value(&c).unwrap();
        assert_eq!(
            json,
            json!({"all":[
                {"prop":"custom.level","op":"gte","value":10},
                {"any":[
                    {"prop":"default.country","op":"in","value":["DE","FR"]},
                    {"prop":"computed.session_count","op":"gt","value":3}
                ]},
                {"not":{"prop":"custom.beta","op":"eq","value":true}},
                {"prop":"computed.last_seen_at","op":"within_last","value":604800}
            ]})
        );
        let text = format(&c);
        assert_eq!(text, src);
        assert_eq!(parse(&text).unwrap(), c);
    }

    #[test]
    fn exists_and_empty() {
        assert_eq!(format(&parse("").unwrap()), "true");
        let c = parse("custom.vip exists and default.app_version semver_gte '1.2.0'").unwrap();
        assert_eq!(
            format(&c),
            r#"custom.vip exists and default.app_version semver_gte "1.2.0""#
        );
        assert!(parse("custom.x ~~ 1").is_err());
        assert!(parse("custom.x == ").is_err());
        assert!(parse("(custom.x == 1").is_err());
    }
}
