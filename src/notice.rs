use std::fmt;

use crate::sh::Result;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Tone {
    Done,
    Moving,
    You,
    NotNow,
    Failed,
}

const TONES: [Tone; 5] = [Tone::Done, Tone::Moving, Tone::You, Tone::NotNow, Tone::Failed];

impl Tone {
    pub fn glyph(self) -> &'static str {
        match self {
            Tone::Done => "✓",
            Tone::Moving => "↻",
            Tone::You => "!",
            Tone::NotNow => "–",
            Tone::Failed => "✗",
        }
    }

    // A notice that asks nothing of the user can go by itself; one that does stays until it is closed.
    pub fn passes(self) -> bool {
        matches!(self, Tone::Done | Tone::Moving | Tone::NotNow)
    }
}

// What a step tells the user: a headline, what happens next, what the user must do (only when they must) and the details behind it, written as lines the terminal shows as they are and the board reads back.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub tone: Tone,
    pub headline: String,
    pub next: Option<String>,
    pub you: Option<String>,
    pub details: Vec<(String, String)>,
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

impl Notice {
    pub fn new(tone: Tone, headline: impl AsRef<str>) -> Notice {
        Notice { tone, headline: crate::status::with_titles(&one_line(headline.as_ref())), next: None, you: None, details: Vec::new() }
    }

    pub fn next(mut self, text: impl AsRef<str>) -> Notice {
        self.next = Some(crate::status::with_titles(&one_line(text.as_ref()))).filter(|text| !text.is_empty());
        self
    }

    pub fn you(mut self, text: impl AsRef<str>) -> Notice {
        self.you = Some(crate::status::with_titles(&one_line(text.as_ref()))).filter(|text| !text.is_empty());
        self
    }

    // The task detail names the task as the board shows it, its title with the name kebbie knows it by.
    pub fn detail(mut self, label: &str, value: impl AsRef<str>) -> Notice {
        let value = value.as_ref().trim();
        if !value.is_empty() {
            let value = match (label, crate::lead::state_location()) {
                ("task", Ok(state)) => crate::status::named(&state, value),
                _ => value.to_string(),
            };
            self.details.push((label.to_string(), value));
        }
        self
    }

    pub fn ok(self) -> Result<String> {
        Ok(self.to_string())
    }

    pub fn err<T>(self) -> Result<T> {
        Err(self.to_string())
    }

    pub fn parse(text: &str) -> Option<Notice> {
        let mut lines = text.lines();
        let first = lines.next()?;
        let tone = TONES.into_iter().find(|tone| first.starts_with(&format!("{} ", tone.glyph())))?;
        let mut notice = Notice::new(tone, &first[tone.glyph().len() + 1..]);
        for line in lines {
            if let Some(more) = line.strip_prefix("    ") {
                match notice.details.last_mut() {
                    Some((_, value)) => {
                        value.push('\n');
                        value.push_str(more);
                    }
                    None => return None,
                }
                continue;
            }
            let (label, value) = line.strip_prefix("  ")?.split_once(": ")?;
            match label {
                "next" => notice.next = Some(value.to_string()),
                "you" => notice.you = Some(value.to_string()),
                _ => notice.details.push((label.to_string(), value.to_string())),
            }
        }
        Some(notice)
    }

    // Any text read as a notice: kebbie's own as written, anything else (a git or herdr error) under a plain headline with the text as its detail.
    pub fn read(text: &str, failed: bool) -> Notice {
        if let Some(notice) = Notice::parse(text) {
            return notice;
        }
        let text = text.trim();
        let short = !text.contains('\n') && text.chars().count() <= 90;
        match (failed, short) {
            (_, true) => Notice::new(if failed { Tone::Failed } else { Tone::Done }, text),
            (true, false) => Notice::new(Tone::Failed, "That did not work").detail("error", text),
            (false, false) => {
                let (first, rest) = text.split_once('\n').unwrap_or((text, ""));
                Notice::new(Tone::Done, first).detail("more", rest)
            }
        }
    }
}

impl fmt::Display for Notice {
    fn fmt(&self, out: &mut fmt::Formatter) -> fmt::Result {
        write!(out, "{} {}", self.tone.glyph(), self.headline)?;
        if let Some(next) = &self.next {
            write!(out, "\n  next: {next}")?;
        }
        if let Some(you) = &self.you {
            write!(out, "\n  you: {you}")?;
        }
        for (label, value) in &self.details {
            write!(out, "\n  {label}: {}", value.lines().collect::<Vec<_>>().join("\n    "))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_notice_reads_back_from_the_lines_the_terminal_shows() {
        let notice = Notice::new(Tone::Moving, "Not pushed yet: main changed the same files")
            .next("Its Claude runs those tests now.").you("Nothing.").detail("files", "a.rs · b.rs").detail("commits", "a1 one\nb2 two");
        let text = notice.to_string();
        assert_eq!(text, "↻ Not pushed yet: main changed the same files\n  next: Its Claude runs those tests now.\n  you: Nothing.\n  files: a.rs · b.rs\n  commits: a1 one\n    b2 two");
        assert_eq!(Notice::parse(&text), Some(notice));
    }

    #[test]
    fn text_that_is_no_notice_still_reads_as_one_with_its_words_kept() {
        assert_eq!(Notice::parse("git push failed: rejected"), None);
        let long = Notice::read("git push failed:\n ! [rejected] main -> main (fetch first)", true);
        assert_eq!((long.tone, long.headline.as_str()), (Tone::Failed, "That did not work"));
        assert!(long.details[0].1.contains("(fetch first)"));
        assert_eq!(Notice::read("Sent to billing.", false).headline, "Sent to billing.");
    }
}
