//! Capture days named without a year or relative to the library: 「10月2日」「上周六」「昨天」,
//! "photos from Oct 2", "last Saturday", "yesterday".
//!
//! Nothing here reads the wall clock. A library may be years old, and a trip shot in October is
//! still "last Saturday" to the user who imports it in March, so every phrase is resolved
//! against the capture days of the session the request is about (`days`: the sorted, distinct
//! local capture days, as days since 1970-01-01; empty = unknown):
//!
//! - **month and day** (「10月2日」, "Oct 2", "2nd of October"): the most recent year of the
//!   session that has photos on that day; with no such year, the latest year of the session in
//!   which the date exists (29 February skips years that have none);
//! - **relative days** (「今天 / 昨天 / 前天」, today / yesterday / the day before yesterday):
//!   counted back from the *reference day*, the latest capture day of the session;
//! - **weeks** run Monday to Sunday. 「上周六」 / 「这周三」 / 「上周」 / 「上周末」 are in the
//!   week before / of the reference day; English "last Saturday" is the most recent such weekday
//!   *before* the reference day, "this Wednesday" the one in the reference week, "last week",
//!   "last weekend", "this week", "this weekend" as in Chinese;
//! - **months**: 「上个月 / 这个月」, last month / this month, are the calendar months before /
//!   of the reference day.
//!
//! A phrase that cannot be resolved (no photos with a capture day, a date that exists in no
//! year of the session) is returned in `unresolved`, and the rules add a hint instead of
//! filtering.

use std::sync::OnceLock;

use regex::Regex;

use crate::library_timeline::{days_from_civil, format_ymd};

/// A day phrase the rules resolved: day numbers (days since 1970-01-01), both ends inclusive,
/// and the byte span of the phrase in the normalised text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mention {
    pub phrase: String,
    pub from: i64,
    pub to: i64,
    pub start: usize,
    pub end: usize,
}

#[derive(Debug, Default)]
pub struct Read {
    pub found: Vec<Mention>,
    /// Phrases that name a day but could not be resolved against the session.
    pub unresolved: Vec<(String, usize, usize)>,
}

#[derive(Debug, Clone, Copy)]
enum Rel {
    /// Days back from the reference day (0 = the reference day itself).
    Back(i64),
    /// A weekday (0 = Monday) of the week `weeks` weeks before the reference week; English
    /// "last <weekday>" is `Last` instead.
    Weekday {
        weeks: i64,
        wd: i64,
    },
    Last(i64),
    /// A whole week or the weekend (`weekend`) `weeks` weeks before the reference week.
    Week {
        weeks: i64,
        weekend: bool,
    },
    /// A calendar month `months` months before the reference day's.
    Month(i64),
}

struct Rx {
    zh_md: Regex,
    en_md1: Regex,
    en_md2: Regex,
    zh_day: Regex,
    zh_week: Regex,
    zh_month: Regex,
    en_day: Regex,
    en_rel: Regex,
}

const EN_MONTH: &str = r"jan(?:uary)?|feb(?:ruary)?|mar(?:ch)?|apr(?:il)?|may|june?|july?|aug(?:ust)?|sep(?:t(?:ember)?)?|oct(?:ober)?|nov(?:ember)?|dec(?:ember)?";
const EN_WEEKDAY: &str = r"mon(?:day)?|tue(?:s(?:day)?)?|wed(?:nesday)?|thu(?:r(?:s(?:day)?)?)?|fri(?:day)?|sat(?:urday)?|sun(?:day)?";

fn rx() -> &'static Rx {
    static RX: OnceLock<Rx> = OnceLock::new();
    RX.get_or_init(|| {
        let r = |p: &str| Regex::new(p).unwrap_or_else(|e| panic!("bad regex {p}: {e}"));
        let n = r"(\d{1,2}|[零〇一二两三四五六七八九十]{1,3})";
        Rx {
            zh_md: r(&format!(r"{n}\s*月\s*{n}\s*[日号]")),
            en_md1: r(&format!(
                r"\b({EN_MONTH})\.?\s+(\d{{1,2}})(?:st|nd|rd|th)?\b(?:\s+(\d{{4}})\b)?"
            )),
            en_md2: r(&format!(
                r"\b(\d{{1,2}})(?:st|nd|rd|th)?\s+(?:of\s+)?({EN_MONTH})\b\.?(?:\s+(\d{{4}})\b)?"
            )),
            zh_day: r("大前天|前天|昨天|昨日|今天|今日"),
            zh_week: r("([上这本])(?:个)?(?:周|星期|礼拜)([一二三四五六日天末])?"),
            zh_month: r("(上个?月|这个?月|本月)"),
            en_day: r(r"\b(day before yesterday|yesterday|today)\b"),
            en_rel: r(&format!(
                r"\b(last|previous|this)\s+(weekend|week|month|{EN_WEEKDAY})\b"
            )),
        }
    })
}

fn month_of(name: &str) -> Option<i64> {
    [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ]
    .iter()
    .position(|m| name.starts_with(m))
    .map(|i| i as i64 + 1)
}

fn weekday_of(name: &str) -> Option<i64> {
    ["mon", "tue", "wed", "thu", "fri", "sat", "sun"]
        .iter()
        .position(|m| name.starts_with(m))
        .map(|i| i as i64)
}

/// A number written with digits or Chinese numerals (「十」「二十一」).
fn number(s: &str) -> Option<i64> {
    if let Ok(n) = s.parse::<i64>() {
        return Some(n);
    }
    let digit = |c: char| "零一二三四五六七八九".chars().position(|d| d == c);
    let (mut total, mut cur) = (0i64, 0i64);
    for c in s.chars() {
        match c {
            '〇' => cur = 0,
            '两' => cur = 2,
            '十' => {
                total += if cur == 0 { 10 } else { cur * 10 };
                cur = 0;
            }
            c => cur = digit(c)? as i64,
        }
    }
    Some(total + cur)
}

/// (year, month, day) of a day number.
pub fn civil(day: i64) -> (i64, i64, i64) {
    let s = format_ymd(day);
    let p = |r: std::ops::Range<usize>| s[r].parse().unwrap_or(0);
    (p(0..4), p(5..7), p(8..10))
}

/// The day number of a real calendar date.
fn real_day(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let day = days_from_civil(y, m, d);
    (civil(day) == (y, m, d)).then_some(day)
}

/// Monday of the week of a day.
fn week_start(day: i64) -> i64 {
    day - (day + 3).rem_euclid(7)
}

/// 0 = Monday.
fn weekday(day: i64) -> i64 {
    (day + 3).rem_euclid(7)
}

/// First and last day of the calendar month `months` months from the one of `day`.
fn month_range(day: i64, months: i64) -> (i64, i64) {
    let (y, m, _) = civil(day);
    let k = y * 12 + (m - 1) + months;
    let first = days_from_civil(k.div_euclid(12), k.rem_euclid(12) + 1, 1);
    let next = days_from_civil((k + 1).div_euclid(12), (k + 1).rem_euclid(12) + 1, 1);
    (first, next - 1)
}

fn resolve(rel: Rel, reference: i64) -> (i64, i64) {
    let ws = week_start(reference);
    match rel {
        Rel::Back(n) => (reference - n, reference - n),
        Rel::Weekday { weeks, wd } => {
            let d = ws - 7 * weeks + wd;
            (d, d)
        }
        Rel::Last(wd) => {
            let mut d = reference - 1;
            while weekday(d) != wd {
                d -= 1;
            }
            (d, d)
        }
        Rel::Week { weeks, weekend } => {
            let first = ws - 7 * weeks;
            if weekend {
                (first + 5, first + 6)
            } else {
                (first, first + 6)
            }
        }
        Rel::Month(back) => month_range(reference, -back),
    }
}

/// The month-and-day phrase as a day: the most recent year of the session with photos on that
/// day, else the latest year in which the date exists.
fn month_day(m: i64, d: i64, days: &[i64]) -> Option<i64> {
    let mut years: Vec<i64> = days.iter().map(|&x| civil(x).0).collect();
    years.sort_unstable();
    years.dedup();
    years
        .iter()
        .rev()
        .filter_map(|&y| real_day(y, m, d))
        .find(|c| days.binary_search(c).is_ok())
        .or_else(|| years.iter().rev().find_map(|&y| real_day(y, m, d)))
}

/// Every phrase of `norm` (normalised text) that names a day without a full date, before
/// resolving. `skip` = byte spans of full dates, whose month and day are not to be read twice.
fn find(norm: &str, skip: &[(usize, usize)]) -> Vec<(usize, usize, Phrase)> {
    let x = rx();
    let mut out: Vec<(usize, usize, Phrase)> = Vec::new();
    let free = |s: usize, e: usize, out: &[(usize, usize, Phrase)]| {
        !skip
            .iter()
            .copied()
            .chain(out.iter().map(|p| (p.0, p.1)))
            .any(|(a, b)| s < b && a < e)
    };
    let add = |s: usize, e: usize, p: Phrase, out: &mut Vec<(usize, usize, Phrase)>| {
        if free(s, e, out) {
            out.push((s, e, p));
        }
    };
    for c in x.zh_md.captures_iter(norm) {
        let w = c.get(0).expect("match");
        if let (Some(m), Some(d)) = (number(&c[1]), number(&c[2])) {
            add(w.start(), w.end(), Phrase::MonthDay(m, d, None), &mut out);
        }
    }
    for c in x.en_md1.captures_iter(norm) {
        let w = c.get(0).expect("match");
        if let (Some(m), Some(d)) = (month_of(&c[1]), c[2].parse().ok()) {
            let y = c.get(3).and_then(|y| y.as_str().parse().ok());
            add(w.start(), w.end(), Phrase::MonthDay(m, d, y), &mut out);
        }
    }
    for c in x.en_md2.captures_iter(norm) {
        let w = c.get(0).expect("match");
        if let (Some(d), Some(m)) = (c[1].parse().ok(), month_of(&c[2])) {
            let y = c.get(3).and_then(|y| y.as_str().parse().ok());
            add(w.start(), w.end(), Phrase::MonthDay(m, d, y), &mut out);
        }
    }
    for m in x.zh_day.find_iter(norm) {
        let back = match m.as_str() {
            "大前天" => 3,
            "前天" => 2,
            "昨天" | "昨日" => 1,
            _ => 0,
        };
        add(m.start(), m.end(), Phrase::Rel(Rel::Back(back)), &mut out);
    }
    for c in x.zh_week.captures_iter(norm) {
        let w = c.get(0).expect("match");
        let weeks = i64::from(&c[1] == "上");
        let rel = match c.get(2).map(|m| m.as_str()) {
            None => Rel::Week {
                weeks,
                weekend: false,
            },
            Some("末") => Rel::Week {
                weeks,
                weekend: true,
            },
            Some(d) => {
                let pos = "一二三四五六日天"
                    .chars()
                    .position(|x| d.starts_with(x))
                    .unwrap_or(0);
                Rel::Weekday {
                    weeks,
                    wd: pos.min(6) as i64,
                }
            }
        };
        add(w.start(), w.end(), Phrase::Rel(rel), &mut out);
    }
    for m in x.zh_month.find_iter(norm) {
        let back = i64::from(m.as_str().starts_with('上'));
        add(m.start(), m.end(), Phrase::Rel(Rel::Month(back)), &mut out);
    }
    for m in x.en_day.find_iter(norm) {
        let back = match m.as_str() {
            "day before yesterday" => 2,
            "yesterday" => 1,
            _ => 0,
        };
        add(m.start(), m.end(), Phrase::Rel(Rel::Back(back)), &mut out);
    }
    for c in x.en_rel.captures_iter(norm) {
        let w = c.get(0).expect("match");
        let this = &c[1] == "this";
        let what = &c[2];
        let rel = match what {
            "weekend" => Rel::Week {
                weeks: i64::from(!this),
                weekend: true,
            },
            "week" => Rel::Week {
                weeks: i64::from(!this),
                weekend: false,
            },
            "month" => Rel::Month(i64::from(!this)),
            wd => match weekday_of(wd) {
                Some(wd) if this => Rel::Weekday { weeks: 0, wd },
                Some(wd) => Rel::Last(wd),
                None => continue,
            },
        };
        add(w.start(), w.end(), Phrase::Rel(rel), &mut out);
    }
    out.sort_by_key(|p| p.0);
    out
}

#[derive(Debug, Clone, Copy)]
enum Phrase {
    /// month, day, explicit year
    MonthDay(i64, i64, Option<i64>),
    Rel(Rel),
}

/// Whether the text names a day this module resolves (so the caller knows it needs the
/// session's capture days).
pub fn mentions(norm: &str, skip: &[(usize, usize)]) -> bool {
    !find(norm, skip).is_empty()
}

/// Resolves the phrases of `norm` against the capture days of the session (sorted, distinct).
pub fn read(norm: &str, days: &[i64], skip: &[(usize, usize)]) -> Read {
    let mut out = Read::default();
    let reference = days.last().copied();
    for (start, end, p) in find(norm, skip) {
        let resolved = match p {
            Phrase::MonthDay(m, d, Some(y)) => real_day(y, m, d).map(|x| (x, x)),
            Phrase::MonthDay(m, d, None) => month_day(m, d, days).map(|x| (x, x)),
            Phrase::Rel(rel) => reference.map(|r| resolve(rel, r)),
        };
        match resolved {
            Some((from, to)) => out.found.push(Mention {
                phrase: norm[start..end].to_string(),
                from,
                to,
                start,
                end,
            }),
            // "13月40日" is not a date at all; "2月30日" neither
            None if matches!(p, Phrase::MonthDay(m, d, _) if real_day(2000, m, d).is_none()) => {}
            None => out
                .unresolved
                .push((norm[start..end].to_string(), start, end)),
        }
    }
    out
}
