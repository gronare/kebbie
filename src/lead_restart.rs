use crate::keys::{self, Act};
use crate::keeper::REASONS;
use crate::known::{self, Record};
use crate::notice::{Notice, Tone};
use crate::restart::{HELD, HOOKS_GONE, KEBBIE_UPDATE, OUTSIDE_LEAD_DIR};
use crate::sh::Result;
use crate::status::LEAD_LABEL;

pub fn behind_of(record: Option<&Record>) -> Option<(String, String)> {
    let record = record?;
    let to = &record.known.kebbie;
    (record.started.as_deref() != Some(to.as_str())).then(|| (record.started.clone().unwrap_or_else(|| "an earlier kebbie".to_string()), to.clone()))
}

pub fn versions() -> Option<(String, String)> {
    behind_of(known::load_in(&crate::lead::roles_dir().ok()?, LEAD_LABEL).as_ref())
}

struct Cause {
    runs: String,
    on: String,
    short: String,
}

// A kebbie update outranks the other reasons, and a line without reasons comes from a keeper older than this kebbie, so it is the update's.
fn cause(reasons: &[&str]) -> Cause {
    let cause = |runs: &str, on: &str, short: &str| Cause { runs: runs.to_string(), on: on.to_string(), short: short.to_string() };
    let reason = reasons.iter().find(|reason| **reason == KEBBIE_UPDATE).or_else(|| reasons.first()).copied().unwrap_or(KEBBIE_UPDATE);
    if reason == KEBBIE_UPDATE {
        cause("the old kebbie", "on the new kebbie", "Lead on old kebbie")
    } else if claude_reason(reason) {
        cause("an older Claude Code", "on the new Claude Code", "Lead on old Claude Code")
    } else if reason == OUTSIDE_LEAD_DIR {
        cause("outside its own folder", "in its own folder", "Lead outside its folder")
    } else if reason == HOOKS_GONE {
        cause("hooks of a kebbie that is gone", "with working hooks", "Lead hooks broken")
    } else if reason == "MCP servers changed" {
        cause("MCP servers from before they changed", "with the changed MCP servers", "Lead on old MCP servers")
    } else if let Some(names) = reason.strip_prefix("plugin ").and_then(|rest| rest.strip_suffix(" updated")) {
        let plugin = if names.contains(", ") { "plugins" } else { "plugin" };
        cause(&format!("the old {plugin} {names}"), &format!("with the updated {plugin} {names}"), &format!("Lead on old {names}"))
    } else {
        cause("older code", "on the new code", "Lead on older code")
    }
}

fn claude_reason(reason: &str) -> bool {
    reason.starts_with("Claude Code ") || reason == "a newer Claude Code"
}

// A restart that left these standing would leave them again, so the line names what mends them instead of the key.
fn unmendable(reasons: &[&str]) -> [String; 2] {
    if reasons.iter().any(|reason| claude_reason(reason)) {
        ["↻ The lead's shell starts the same older Claude Code, so a restart cannot update it · put the new one first on that shell's PATH".to_string(), "↻ Lead on old Claude Code · fix PATH".to_string()]
    } else if reasons.contains(&HOOKS_GONE) {
        ["↻ The lead's hooks still run a kebbie that is gone after a restart · kebbie doctor shows the fix".to_string(), "↻ Lead hooks broken · kebbie doctor".to_string()]
    } else {
        ["↻ The lead stays stale after a restart and another would not mend it · kebbie restart says why".to_string(), "↻ Lead stuck stale · kebbie restart".to_string()]
    }
}

// `stale` is the restart routine's "label: said" lines with their reasons after keeper::REASONS; the line shows only while the lead is among them.
pub fn line(stale: &[String], auto: bool, width: usize) -> Option<String> {
    let entry = stale.iter().find_map(|line| line.strip_prefix(LEAD_LABEL).and_then(|rest| rest.strip_prefix(": ")))?;
    let (said, reasons) = entry.split_once(REASONS).map_or((entry, Vec::new()), |(said, reasons)| (said, reasons.split("; ").collect::<Vec<_>>()));
    if said == "restarted" {
        return None;
    }
    let key = keys::map().label(Act::RestartLead);
    let cause = cause(&reasons);
    let held = said.strip_prefix(HELD);
    let wordings = if held.is_some_and(|why| why.starts_with("a restart onto ")) {
        [format!("↻ A restart left the lead on the old kebbie · {key} tries again"), format!("↻ Lead still on old kebbie · {key} retries")]
    } else if held.is_some_and(|why| !why.starts_with("restarted ")) {
        unmendable(&reasons)
    } else if said.starts_with("restart failed") {
        [format!("↻ The lead's restart {} failed · {key} tries again", cause.on), format!("↻ Lead restart failed · {key} retries")]
    } else if !auto || said.contains("run kebbie restart") {
        [format!("↻ The lead runs {} until you restart it · {key} restarts it", cause.runs), format!("↻ {} · {key} restarts", cause.short)]
    } else if said.starts_with("you are in it") {
        [format!("↻ The lead restarts {} when you leave it · {key} restarts it now", cause.on), format!("↻ Lead restarts when you leave · {key} now")]
    } else {
        [format!("↻ The lead restarts {} when it's idle · {key} restarts it now", cause.on), format!("↻ Lead restarts when idle · {key} now")]
    };
    let [long, short] = wordings;
    Some(if crate::board::widgets::text_width(&long) <= width { long } else { short })
}

pub const CONTEXT_FULL: f64 = 85.0;

// The lead's context share from its status line; nothing below the mark or when the share is unknown.
pub fn context_line(used: Option<f64>, width: usize) -> Option<String> {
    let used = used.filter(|used| *used >= CONTEXT_FULL)?.round().min(100.0);
    let key = keys::map().label(Act::RestartLead);
    let long = format!("The lead's context is {used}% full · {key} to restart it fresh");
    Some(if crate::board::widgets::text_width(&long) <= width { long } else { format!("Lead context {used}% · {key}") })
}

pub fn outcome(ran: std::result::Result<String, String>) -> Result<String> {
    match ran {
        Ok(said) => Ok(said),
        Err(why) if why.starts_with(crate::lead::REFUSED) => Ok(Notice::new(Tone::NotNow, why).next(format!("{} tries again.", keys::map().label(Act::RestartLead))).to_string()),
        Err(error) if error.is_empty() => Err("kebbie lead --restart failed".to_string()),
        Err(error) => Err(error),
    }
}

// Through kebbie_command, so the start records the version that runs it.
pub fn restart_now() -> Result<String> {
    outcome(crate::sh::attempt(&crate::peers::kebbie_command()?, &["lead", "--restart"], None)?)
}

pub fn fresh_now() -> Result<String> {
    outcome(crate::sh::attempt(&crate::peers::kebbie_command()?, &["lead", "--restart", "--fresh"], None)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::known::Snapshot;

    fn told(started: Option<&str>, known: &str) -> Record {
        Record { started: started.map(str::to_string), known: Snapshot { kebbie: known.to_string(), ..Snapshot::default() } }
    }

    #[test]
    fn the_versions_differ_from_the_update_until_the_lead_runs_the_kebbie_it_was_told_of() {
        assert_eq!(behind_of(Some(&told(Some("kebbie 0.26.1"), "kebbie 0.26.2"))), Some(("kebbie 0.26.1".to_string(), "kebbie 0.26.2".to_string())));
        assert_eq!(behind_of(Some(&told(None, "kebbie 0.26.2"))), Some(("an earlier kebbie".to_string(), "kebbie 0.26.2".to_string())));
        assert_eq!(behind_of(None), None);
        let dir = crate::sh::test_dir("lead-restart-record");
        let snapshot = |kebbie: &str| Snapshot { kebbie: kebbie.to_string(), ..Snapshot::default() };
        known::began_in(&dir, LEAD_LABEL, &snapshot("kebbie 0.26.2"), true, &mut |_| Ok(())).unwrap();
        assert_eq!(behind_of(known::load_in(&dir, LEAD_LABEL).as_ref()), None, "the restart records the kebbie it started on");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_board_line_shows_while_the_lead_is_stale_and_says_what_happens_and_which_key_restarts_it() {
        let stale = |said: &str| vec!["billing@app: restarts when idle".to_string(), format!("lead: {said}{REASONS}{KEBBIE_UPDATE}")];
        assert_eq!(line(&["billing@app: restarts when idle".to_string()], true, 120), None, "no line while the lead is not stale");
        assert_eq!(line(&stale("restarted"), true, 120), None, "the round that restarted it shows no line");
        assert_eq!(line(&stale("restarts when idle"), true, 120).unwrap(), "↻ The lead restarts on the new kebbie when it's idle · L restarts it now");
        assert_eq!(line(&stale("not restarted yet: the lead is working"), true, 120).unwrap(), "↻ The lead restarts on the new kebbie when it's idle · L restarts it now");
        assert_eq!(line(&stale("you are in it: restarts when you leave it"), true, 120).unwrap(), "↻ The lead restarts on the new kebbie when you leave it · L restarts it now");
        assert_eq!(line(&stale("restart failed: Claude did not come back"), true, 120).unwrap(), "↻ The lead's restart on the new kebbie failed · L tries again");
        assert_eq!(line(&stale("stale: run kebbie restart when you want it"), false, 120).unwrap(), "↻ The lead runs the old kebbie until you restart it · L restarts it", "with restart.auto off nothing restarts it on its own");
        for said in ["restarts when idle", "you are in it: restarts when you leave it", "restart failed: Claude did not come back", "stale: run kebbie restart when you want it"] {
            for width in [76, 116, 40] {
                let shown = line(&stale(said), true, width).unwrap();
                assert!(crate::board::widgets::text_width(&shown) <= width, "{shown} is cut at {width}");
                assert!(!shown.contains('(') && !shown.contains("2026"), "no commit or date: {shown}");
            }
        }
    }

    fn lead(said: &str, reasons: &[&str]) -> Vec<String> {
        crate::keeper::stale_lines(&[crate::restart::Line { label: LEAD_LABEL.to_string(), said: said.to_string(), stale: true, reasons: reasons.iter().map(|reason| reason.to_string()).collect() }])
    }

    #[test]
    fn the_line_names_why_the_lead_is_stale_and_says_old_kebbie_only_for_a_kebbie_update() {
        let manual = crate::restart::Verdict::Manual.words(false);
        assert_eq!(line(&lead(&manual, &["MCP servers changed"]), false, 120).unwrap(), "↻ The lead runs MCP servers from before they changed until you restart it · L restarts it");
        assert_eq!(line(&lead(&manual, &[OUTSIDE_LEAD_DIR]), false, 120).unwrap(), "↻ The lead runs outside its own folder until you restart it · L restarts it");
        assert_eq!(line(&lead(&manual, &["a newer Claude Code"]), false, 120).unwrap(), "↻ The lead runs an older Claude Code until you restart it · L restarts it");
        assert_eq!(line(&lead(&manual, &["plugin vault updated"]), false, 120).unwrap(), "↻ The lead runs the old plugin vault until you restart it · L restarts it");
        assert_eq!(line(&lead(&manual, &[HOOKS_GONE]), false, 120).unwrap(), "↻ The lead runs hooks of a kebbie that is gone until you restart it · L restarts it");
        assert_eq!(line(&lead("restarts when idle", &["plugin vault, kebbie updated"]), true, 120).unwrap(), "↻ The lead restarts with the updated plugins vault, kebbie when it's idle · L restarts it now");
        assert_eq!(line(&lead(&manual, &["MCP servers changed", KEBBIE_UPDATE]), false, 120).unwrap(), "↻ The lead runs the old kebbie until you restart it · L restarts it", "a kebbie update is named first");
        assert_eq!(line(&lead(&manual, &["it runs without its task role"]), false, 120).unwrap(), "↻ The lead runs older code until you restart it · L restarts it", "a reason without its own wording claims no kebbie update");
        assert_eq!(line(&lead("restarted", &[KEBBIE_UPDATE]), true, 120), None, "the reasons do not keep the restarted round's line up");
        for reasons in [["MCP servers changed"], [OUTSIDE_LEAD_DIR], ["a newer Claude Code"], [HOOKS_GONE], [KEBBIE_UPDATE]] {
            let shown = line(&lead(&manual, &reasons), false, 40).unwrap();
            assert!(crate::board::widgets::text_width(&shown) <= 40, "{shown} is cut at 40");
            assert_eq!(shown.contains("kebbie"), reasons == [KEBBIE_UPDATE], "{shown}");
        }
    }

    #[test]
    fn a_lead_held_for_a_reason_a_restart_cannot_mend_says_what_mends_it_and_offers_no_l() {
        let claude = format!("{HELD}a restart brought back the same Claude Code (/opt/devbox/claude-2.1.296/bin/claude), so the pane starts that one; restarting again cannot update it");
        for auto in [true, false] {
            let shown = line(&lead(&claude, &["a newer Claude Code"]), auto, 200).unwrap();
            assert_eq!(shown, "↻ The lead's shell starts the same older Claude Code, so a restart cannot update it · put the new one first on that shell's PATH");
            assert!(!shown.contains("L ") && !shown.contains("old kebbie"), "{shown}");
        }
        let hooks = format!("{HELD}its hooks run a kebbie that is gone after a restart already; restarting again would not mend it");
        assert_eq!(line(&lead(&hooks, &[HOOKS_GONE]), false, 200).unwrap(), "↻ The lead's hooks still run a kebbie that is gone after a restart · kebbie doctor shows the fix");
        assert_eq!(line(&lead(&claude, &["a newer Claude Code"]), false, 40).unwrap(), "↻ Lead on old Claude Code · fix PATH");
        let cooldown = format!("{HELD}restarted 1 min ago; the next restart waits 30 min");
        assert_eq!(line(&lead(&cooldown, &["MCP servers changed"]), false, 120).unwrap(), "↻ The lead runs MCP servers from before they changed until you restart it · L restarts it", "L skips the cooldown");
        let stuck = format!("{HELD}a restart onto kebbie 0.30.0 left it on kebbie 0.29.0; restarting again would not move it");
        assert_eq!(line(&lead(&stuck, &[KEBBIE_UPDATE]), true, 120).unwrap(), "↻ A restart left the lead on the old kebbie · L tries again", "L restarts by hand, which this hold lets through");
    }

    #[test]
    fn a_refused_restart_shows_its_plain_reason_and_a_failure_stays_an_error() {
        let refused = outcome(Err("Not restarted: the lead is working.".to_string())).unwrap();
        assert!(refused.contains("Not restarted: the lead is working.") && refused.contains("L tries again") && !refused.contains("kebbie lead --restart"), "{refused}");
        assert_eq!(Notice::parse(&refused).map(|notice| notice.tone), Some(Tone::NotNow));
        assert_eq!(outcome(Err("Claude did not come back in pane p_9".to_string())), Err("Claude did not come back in pane p_9".to_string()));
        assert_eq!(outcome(Ok("Restarted the lead.".to_string())), Ok("Restarted the lead.".to_string()));
    }

    #[test]
    fn the_context_line_shows_only_once_the_leads_context_is_nearly_full_and_never_when_unknown() {
        assert_eq!(context_line(None, 120), None, "an unknown share shows nothing");
        assert_eq!(context_line(Some(84.4), 120), None);
        assert_eq!(context_line(Some(87.4), 120).unwrap(), "The lead's context is 87% full · L to restart it fresh");
        assert_eq!(context_line(Some(85.0), 30).unwrap(), "Lead context 85% · L", "a narrow board takes the short wording");
        assert_eq!(context_line(Some(103.0), 120).unwrap(), "The lead's context is 100% full · L to restart it fresh");
    }
}
