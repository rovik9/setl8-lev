//! Weekly session windows (DESIGN 3.4, 3.5; decision 49: the windows alone decide open or closed).
//!
//! A market's sessions are described as one or more VARIANTS (for example the summer and the winter hours of an exchange);
//! each variant is a list of segments. The generator takes the INTERSECTION of the variants, so where the data and the
//! calendar do not settle a boundary the NARROWER window wins. Minutes of the UTC week: Monday 00:00 = 0, windows are
//! `[start, end)` and never wrap past 10080, so a segment that crosses Sunday midnight becomes two windows.

pub const MINUTES_PER_WEEK: u16 = 10_080;
pub const MAX_SESSIONS: usize = 6;
const DAY: u16 = 1_440;
const DAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn day_of(name: &str) -> Result<u16, String> {
    DAYS.iter().position(|d| *d == name).map(|i| i as u16).ok_or_else(|| format!("unknown day {name:?} (Mon Tue Wed Thu Fri Sat Sun)"))
}

/// `HH:MM` to minutes of the day; `24:00` is allowed (an end of day).
fn clock(s: &str) -> Result<u16, String> {
    let (h, m) = s.split_once(':').ok_or_else(|| format!("time {s:?} is not HH:MM"))?;
    let (h, m): (u16, u16) = (h.parse().map_err(|_| format!("hour in {s:?}"))?, m.parse().map_err(|_| format!("minute in {s:?}"))?);
    if m > 59 || h > 24 || (h == 24 && m != 0) {
        return Err(format!("time {s:?} is out of range"));
    }
    Ok(h * 60 + m)
}

/// `Ddd HH:MM` to a minute of the week (`Sun 24:00` = 10080 is a valid END).
fn point(s: &str) -> Result<u16, String> {
    let (d, t) = s.trim().split_once(' ').ok_or_else(|| format!("{s:?} is not \"Ddd HH:MM\""))?;
    Ok(day_of(d.trim())? * DAY + clock(t.trim())?)
}

/// One segment as week minutes, split at the week end: `Sun 23:00-Mon 21:00` is `[10020,10080)` and `[0,1260)`.
/// Forms: `24x7`; `Ddd HH:MM-Ddd HH:MM`; `Ddd-Ddd HH:MM-HH:MM` (the same clock window on each day of a range of days, no wrap).
pub fn segment(spec: &str) -> Result<Vec<(u16, u16)>, String> {
    let spec = spec.trim();
    if spec == "24x7" {
        return Ok(vec![(0, MINUTES_PER_WEEK)]);
    }
    let (first, rest) = spec.split_once(' ').ok_or_else(|| format!("segment {spec:?} has no time"))?;
    if let Some((d1, d2)) = first.split_once('-') {
        // `Mon-Fri 14:30-20:00`
        let (a, b) = (day_of(d1)?, day_of(d2)?);
        let (t1, t2) = rest.split_once('-').ok_or_else(|| format!("{spec:?}: expected HH:MM-HH:MM"))?;
        let (s, e) = (clock(t1.trim())?, clock(t2.trim())?);
        if a > b || s >= e {
            return Err(format!("{spec:?}: the days must ascend and the window must not be empty or wrap"));
        }
        return Ok((a..=b).map(|d| (d * DAY + s, d * DAY + e)).collect());
    }
    // `Mon 23:00-Tue 21:00`: split on the dash that is followed by a day name
    let at = (1..spec.len()).find(|&i| spec.is_char_boundary(i) && spec.as_bytes()[i] == b'-' && day_of(spec[i + 1..].trim().split(' ').next().unwrap_or("")).is_ok());
    let at = at.ok_or_else(|| format!("segment {spec:?} is not \"Ddd HH:MM-Ddd HH:MM\""))?;
    let (s, e) = (point(&spec[..at])?, point(&spec[at + 1..])?);
    match e.cmp(&s) {
        std::cmp::Ordering::Equal => Err(format!("segment {spec:?} is empty (use 24x7 for the whole week)")),
        std::cmp::Ordering::Greater => Ok(vec![(s, e)]),
        // the end is earlier in the week than the start: the segment crosses the week end (Sunday midnight)
        std::cmp::Ordering::Less if e == 0 => Ok(vec![(s, MINUTES_PER_WEEK)]),
        std::cmp::Ordering::Less => Ok(vec![(s, MINUTES_PER_WEEK), (0, e)]),
    }
}

fn bitmap(variant: &[String]) -> Result<Vec<bool>, String> {
    let mut open = vec![false; usize::from(MINUTES_PER_WEEK)];
    for seg in variant {
        for (s, e) in segment(seg)? {
            open[usize::from(s)..usize::from(e)].iter_mut().for_each(|m| *m = true);
        }
    }
    Ok(open)
}

/// The windows of the intersection of the variants, ascending, merged where they touch. At most [`MAX_SESSIONS`] and at least one.
pub fn windows(variants: &[Vec<String>]) -> Result<Vec<(u16, u16)>, String> {
    if variants.is_empty() {
        return Err("a market needs at least one session variant".into());
    }
    let mut open = bitmap(&variants[0])?;
    for v in &variants[1..] {
        let other = bitmap(v)?;
        open.iter_mut().zip(other).for_each(|(a, b)| *a = *a && b);
    }
    let mut out = Vec::new();
    let mut start: Option<u16> = None;
    for (m, &is_open) in open.iter().enumerate() {
        let m = m as u16;
        match (is_open, start) {
            (true, None) => start = Some(m),
            (false, Some(s)) => {
                out.push((s, m));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        out.push((s, MINUTES_PER_WEEK));
    }
    if out.is_empty() {
        return Err("the session variants have no minute in common".into());
    }
    if out.len() > MAX_SESSIONS {
        return Err(format!("the sessions need {} windows, the market holds {MAX_SESSIONS}", out.len()));
    }
    Ok(out)
}

/// `Mon 23:00-Tue 21:00, ...` for launch.md; the week end prints as `Sun 24:00`.
pub fn describe(w: &[(u16, u16)]) -> String {
    if w == [(0, MINUTES_PER_WEEK)] {
        return "24/7".into();
    }
    let at = |m: u16, is_end: bool| {
        let (d, r) = if is_end && m > 0 && m.is_multiple_of(DAY) { (m / DAY - 1, DAY) } else { (m / DAY, m % DAY) };
        format!("{} {:02}:{:02}", DAYS[usize::from(d.min(6))], r / 60, r % 60)
    };
    w.iter().map(|&(s, e)| format!("{}-{}", at(s, false), at(e, true))).collect::<Vec<_>>().join(", ")
}

/// Windows of a market as written to the chain: six slots, unused ones `(0, 0)`.
pub fn padded(w: &[(u16, u16)]) -> [(u16, u16); MAX_SESSIONS] {
    let mut out = [(0, 0); MAX_SESSIONS];
    for (slot, win) in out.iter_mut().zip(w) {
        *slot = *win;
    }
    out
}
