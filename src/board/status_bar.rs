use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};

use crate::keys::{self, Act, Ctx};
use crate::usage::Usage;
use crate::keys::Hints;
use super::Hit;
use super::widgets::{FAINT, GREEN, MUTED, RED, SUBTEXT, YELLOW, style};

fn status_hints(keys: &Hints) -> Line<'static> {
    Line::from(Span::styled(keys.iter().map(|(key, label, _)| format!("{key} {label}")).collect::<Vec<_>>().join(" · "), style(MUTED)))
}

fn work_hints(opens: Option<&'static str>, lead: bool) -> (Hints, Hints) {
    let (mut full, mut short) = keys::map().hints(Ctx::Work, lead);
    if let Some(opens) = opens {
        let entry = (keys::key(Act::Card).to_string(), opens, keys::map().code(Act::Card));
        full.insert(1, entry.clone());
        short.insert(1, entry);
    }
    (full, short)
}

pub(super) fn hint_lines(opens: Option<&'static str>, lead: bool) -> (Line<'static>, Line<'static>) {
    let (full, short) = work_hints(opens, lead);
    (status_hints(&full), status_hints(&short))
}

pub(super) fn hint_hits(line: &Line, area: Rect, opens: Option<&'static str>, lead: bool) -> Vec<(Rect, Hit)> {
    let text = line.to_string();
    let mut hits = Vec::new();
    let update = keys::map().code(Act::Update);
    let update_hints = [(update_hint(), update), (short_update_hint(), update)];
    for (hint, code) in work_hints(opens, lead).0.into_iter().map(|(key, label, code)| (format!("{key} {label}"), code)).chain(update_hints) {
        let Some(byte) = text.find(&hint) else { continue };
        let x = Span::raw(&text[..byte]).width() as u16;
        let width = Span::raw(hint.as_str()).width() as u16;
        if x < area.width {
            hits.push((Rect::new(area.x + x, area.y, width.min(area.width - x), 1), Hit::Key(code)));
        }
    }
    hits
}

fn local(at: u64) -> Option<libc::tm> {
    let time = at as libc::time_t;
    // SAFETY: localtime_r only writes the tm it is given.
    unsafe {
        let mut tm: libc::tm = std::mem::zeroed();
        (!libc::localtime_r(&time, &mut tm).is_null()).then_some(tm)
    }
}

pub(super) fn clock(at: u64) -> String {
    local(at).map_or_else(String::new, |tm| format!("{:02}:{:02}", tm.tm_hour, tm.tm_min))
}

fn resets(at: u64, now: u64) -> String {
    match (local(at), local(now)) {
        (Some(then), Some(today)) if (then.tm_year, then.tm_yday) == (today.tm_year, today.tm_yday) => format!("{:02}:{:02}", then.tm_hour, then.tm_min),
        (Some(then), _) => ["sun", "mon", "tue", "wed", "thu", "fri", "sat"][then.tm_wday.clamp(0, 6) as usize].to_string(),
        _ => String::new(),
    }
}

fn usage_color(used: f64) -> Color {
    if used < 50.0 { GREEN } else if used < 80.0 { YELLOW } else { RED }
}

fn usage_spans(usage: &Usage, wide: bool, now: u64) -> Vec<Span<'static>> {
    let windows = [(if wide { "session" } else { "5h" }, &usage.session), (if wide { "week" } else { "7d" }, &usage.week)];
    let mut spans = Vec::new();
    for (label, window) in windows {
        let Some(window) = window else { continue };
        let used = window.used.clamp(0.0, 100.0);
        let color = usage_color(used);
        if !spans.is_empty() {
            spans.push(Span::styled(if wide { "   " } else { " · " }, style(MUTED)));
        }
        spans.push(Span::styled(format!("{label} "), style(SUBTEXT)));
        if wide {
            let filled = (used / 100.0 * 8.0).round() as usize;
            spans.push(Span::styled("━".repeat(filled), style(color)));
            spans.push(Span::styled("━".repeat(8 - filled), style(FAINT)));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(format!("{used:.0}%"), style(color)));
        if wide {
            spans.push(Span::styled(format!(" resets {}", resets(window.resets_at, now)), style(MUTED)));
        }
    }
    spans
}

fn update_notice(updates: &[(String, String)], long: bool) -> Vec<Span<'static>> {
    let text = if long {
        let names: Vec<String> = updates.iter().map(|(name, version)| format!("{name} {version}")).collect();
        format!("↑ {} · {}", names.join(" · "), update_hint())
    } else {
        short_update_hint()
    };
    vec![Span::raw("   "), Span::styled(text, style(MUTED))]
}

fn update_hint() -> String {
    format!("{} updates", keys::key(Act::Update))
}

fn short_update_hint() -> String {
    format!("↑ update · {}", keys::key(Act::Update))
}

pub(super) fn status_line(full: Line<'static>, short: Line<'static>, updates: &[(String, String)], usage: Option<&Usage>, width: usize, now: u64) -> Line<'static> {
    let right = |wide: bool| usage.map(|usage| usage_spans(usage, wide, now)).unwrap_or_default();
    let width_of = |spans: &[Span]| spans.iter().map(|span| span.width()).sum::<usize>();
    let left_width = |line: &Line| line.width() - line.spans.last().map_or(0, |span| span.content.len() - span.content.trim_end().len());
    let notices: Vec<Vec<Span<'static>>> = if updates.is_empty() { Vec::new() } else { vec![update_notice(updates, true), update_notice(updates, false)] };
    let short_text = short.to_string();
    let shortest = Line::from(Span::styled(short_text.rsplit_once(" · ").map_or(short_text.as_str(), |(kept, _)| kept).to_string(), short.spans.first().map_or_else(Style::new, |span| span.style)));
    let combos = [(&full, true), (&short, true), (&full, false), (&short, false)];
    let with_notice: Vec<((&Line, bool), Vec<Span>)> = combos[..2].iter().flat_map(|combo| notices.iter().map(move |notice| (*combo, notice.clone())))
        .chain(notices.last().map(|notice| ((&shortest, true), notice.clone())))
        .chain(combos[2..].iter().flat_map(|combo| notices.iter().map(move |notice| (*combo, notice.clone())))).collect();
    let tries = with_notice.into_iter().chain(combos.iter().map(|combo| (*combo, Vec::new())));
    for ((left, wide), notice) in tries {
        let spans = right(wide);
        let used = left_width(left) + width_of(&notice) + width_of(&spans);
        if used + 2 <= width || spans.is_empty() && used <= width {
            let mut line: Vec<Span> = left.spans.clone();
            if let Some(last) = line.last_mut() {
                last.content = last.content.trim_end().to_string().into();
            }
            line.extend(notice);
            line.push(Span::raw(" ".repeat(width.saturating_sub(used))));
            line.extend(spans);
            return Line::from(line);
        }
    }
    short
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyCode;
    use ratatui::style::Color;

    use super::*;
    use crate::board::test_support::*;
    use crate::messages;
    use crate::usage::Usage;
    use crate::usage;
    use crate::board::{Hit, Page};
    use crate::board::widgets::{FAINT, GREEN, MUTED, RED, SUBTEXT, YELLOW};

    // Ten minutes ahead crosses midnight in the day's last ten minutes, and a reset on another day is drawn as a weekday.
    fn later_today(now: u64) -> u64 {
        if resets(now + 600, now).contains(':') { now + 600 } else { now + 1 }
    }

    #[test]
    fn the_status_bar_shows_plan_usage_as_the_mockup_draws_it_wide_and_short() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let now = messages::now();
        app.usage = Some(Usage {
            session: Some(usage::Window { used: 23.0, resets_at: later_today(now) }), week: Some(usage::Window { used: 71.4, resets_at: now + 3 * 86_400 }), seen_at: now,
        });
        let (rows, buffer) = render(&mut app, 170, 30);
        let bar = &rows[28];
        assert!(bar.trim_start().starts_with("⏎ options · tab lead · i inbox · m messages · ? keys"), "{bar}");
        assert!(bar.contains(&format!("session ━━━━━━━━ 23% resets {}   week ━━━━━━━━ 71% resets ", clock(later_today(now)))), "{bar}");
        let (x, _) = find(&rows[28..29], "session ━")[0];
        let fills: Vec<Color> = (0..8).map(|cell| buffer[(x as u16 + 8 + cell, 28)].fg).collect();
        assert_eq!(fills, [GREEN, GREEN, FAINT, FAINT, FAINT, FAINT, FAINT, FAINT], "23% fills two of eight on a surface1 track");
        assert_eq!(buffer[(x as u16, 28)].fg, SUBTEXT);
        let (x, _) = find(&rows[28..29], "23%")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, GREEN);
        assert_eq!(buffer[(x as u16 + 4, 28)].fg, MUTED, "the reset time is faint");
        let (x, _) = find(&rows[28..29], "71%")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, YELLOW);
        let (rows, _) = render(&mut app, 112, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox · ? keys") && rows[28].contains("session ━━━━━━━━ 23% resets"), "the herdr popup keeps the bars: {}", rows[28]);
        let (rows, _) = render(&mut app, 80, 30);
        assert!(rows[28].trim_end().ends_with("5h 23% · 7d 71%"), "{}", rows[28]);
        app.usage = None;
        let (rows, _) = render(&mut app, 96, 30);
        assert!(!rows[28].contains('%'));
        assert_eq!(usage_color(49.0), GREEN);
        assert_eq!(usage_color(80.0), RED);
    }

    #[test]
    fn a_newer_version_shows_muted_beside_the_hints_without_pushing_out_the_usage_bars() {
        let mut app = test_app(vec![crate::status::tests::task(Some("idle"), None, 0)]);
        let now = messages::now();
        app.usage = Some(Usage {
            session: Some(usage::Window { used: 23.0, resets_at: later_today(now) }), week: Some(usage::Window { used: 71.4, resets_at: now + 3 * 86_400 }), seen_at: now,
        });
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows[28].contains('↑'), "nothing newer, no notice: {}", rows[28]);
        app.updates = vec![("kelpie".to_string(), "0.18.2".to_string())];
        let (rows, _) = render(&mut app, 170, 30);
        assert!(rows[28].contains("m messages · ? keys   ↑ kelpie 0.18.2 · U updates") && rows[28].contains("session ━━━━━━━━ 23% resets"), "{}", rows[28]);
        app.updates.push(("vault".to_string(), "0.2.17".to_string()));
        let (rows, _) = render(&mut app, 163, 30);
        assert!(rows[28].contains("m messages · ? keys   ↑ update · U") && rows[28].contains("session ━━━━━━━━ 23% resets"), "the notice shortens before the hints do: {}", rows[28]);
        let (rows, buffer) = render(&mut app, 200, 30);
        assert!(rows[28].contains("↑ kelpie 0.18.2 · vault 0.2.17 · U updates") && rows[28].contains("session ━━━━━━━━ 23% resets"), "{}", rows[28]);
        let (x, _) = find(&rows[28..29], "↑ kelpie")[0];
        assert_eq!(buffer[(x as u16, 28)].fg, MUTED);
        let (ux, _) = find(&rows[28..29], "U updates")[0];
        assert_eq!(app.hit_at(ux as u16, 28), Some(Hit::Key(KeyCode::Char('U'))));
        let (rows, _) = render(&mut app, 112, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox   ↑ update · U") && rows[28].contains("session ━━━━━━━━ 23% resets") && rows[28].contains("week ━━━━━━━━ 71% resets"), "the herdr popup keeps the bars, ? keys makes room: {}", rows[28]);
        let (x, _) = find(&rows[28..29], "↑ update · U")[0];
        assert_eq!(app.hit_at(x as u16, 28), Some(Hit::Key(KeyCode::Char('U'))));
        let (rows, _) = render(&mut app, 40, 30);
        assert!(rows[28].trim_start().starts_with("⏎ options · i inbox · ? keys"), "too narrow for it, the hints stay: {}", rows[28]);
        app.page = Page::Areas;
        let (rows, _) = render(&mut app, 170, 30);
        assert!(!rows[28].contains("updates") && !rows[28].contains("↑ update"), "the Work page only: {}", rows[28]);
    }
}
