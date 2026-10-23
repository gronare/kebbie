use crate::keys::{self, Act};
use crate::known::{self, Record};
use crate::notice::{Notice, Tone};
use crate::sh::Result;
use crate::status::LEAD_LABEL;

pub fn behind_of(record: Option<&Record>) -> Option<(String, String)> {
    let record = record?;
    let to = &record.known.kelpie;
    (record.started.as_deref() != Some(to.as_str())).then(|| (record.started.clone().unwrap_or_else(|| "an earlier kelpie".to_string()), to.clone()))
}

pub fn versions() -> Option<(String, String)> {
    behind_of(known::load_in(&crate::lead::roles_dir().ok()?, LEAD_LABEL).as_ref())
}

// `stale` is the restart routine's "label: said" lines; the line shows only while the lead is among them.
pub fn line(stale: &[String], versions: Option<&(String, String)>, auto: bool) -> Option<String> {
    let said = stale.iter().find_map(|line| line.strip_prefix(LEAD_LABEL).and_then(|rest| rest.strip_prefix(": "))).filter(|said| *said != "restarted")?;
    let key = keys::map().label(Act::RestartLead);
    let head = match versions {
        Some((from, to)) => format!("↻ The lead runs {from}; {to} is installed"),
        None => "↻ The lead runs older code than is installed".to_string(),
    };
    Some(if said.starts_with("restart failed") {
        format!("{head} · {said} · {key} tries again")
    } else if auto {
        format!("{head} · {said} · {key} restarts it now")
    } else {
        format!("{head} · {key} restarts it")
    })
}

pub fn outcome(ran: std::result::Result<String, String>) -> Result<String> {
    match ran {
        Ok(said) => Ok(said),
        Err(why) if why.starts_with(crate::lead::REFUSED) => Ok(Notice::new(Tone::NotNow, why).next(format!("{} tries again.", keys::map().label(Act::RestartLead))).to_string()),
        Err(error) if error.is_empty() => Err("kelpie lead --restart failed".to_string()),
        Err(error) => Err(error),
    }
}

// Through kelpie_command, so the start records the version that runs it.
pub fn restart_now() -> Result<String> {
    outcome(crate::sh::attempt(&crate::peers::kelpie_command()?, &["lead", "--restart"], None)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::known::Snapshot;

    fn told(started: Option<&str>, known: &str) -> Record {
        Record { started: started.map(str::to_string), known: Snapshot { kelpie: known.to_string(), ..Snapshot::default() } }
    }

    #[test]
    fn the_versions_differ_from_the_update_until_the_lead_runs_the_kelpie_it_was_told_of() {
        assert_eq!(behind_of(Some(&told(Some("kelpie 0.26.1"), "kelpie 0.26.2"))), Some(("kelpie 0.26.1".to_string(), "kelpie 0.26.2".to_string())));
        assert_eq!(behind_of(Some(&told(None, "kelpie 0.26.2"))), Some(("an earlier kelpie".to_string(), "kelpie 0.26.2".to_string())));
        assert_eq!(behind_of(None), None);
        let dir = crate::sh::test_dir("lead-restart-record");
        let snapshot = |kelpie: &str| Snapshot { kelpie: kelpie.to_string(), ..Snapshot::default() };
        known::began_in(&dir, LEAD_LABEL, &snapshot("kelpie 0.26.2"), true, &mut |_| Ok(())).unwrap();
        assert_eq!(behind_of(known::load_in(&dir, LEAD_LABEL).as_ref()), None, "the restart records the kelpie it started on");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_board_line_shows_while_the_lead_is_stale_and_says_what_happens_and_which_key_restarts_it() {
        let versions = ("kelpie 0.26.1".to_string(), "kelpie 0.26.2".to_string());
        let stale = |said: &str| vec!["billing@app: restarts when idle".to_string(), format!("lead: {said}")];
        assert_eq!(line(&["billing@app: restarts when idle".to_string()], Some(&versions), true), None, "no line while the lead is not stale");
        assert_eq!(line(&stale("restarted"), Some(&versions), true), None, "the round that restarted it shows no line");
        assert_eq!(line(&stale("you are in it: restarts when you leave it"), Some(&versions), true).unwrap(), "↻ The lead runs kelpie 0.26.1; kelpie 0.26.2 is installed · you are in it: restarts when you leave it · L restarts it now");
        assert_eq!(line(&stale("restart failed: Claude did not come back"), Some(&versions), true).unwrap(), "↻ The lead runs kelpie 0.26.1; kelpie 0.26.2 is installed · restart failed: Claude did not come back · L tries again");
        assert_eq!(line(&stale("stale: run kelpie restart when you want it"), Some(&versions), false).unwrap(), "↻ The lead runs kelpie 0.26.1; kelpie 0.26.2 is installed · L restarts it", "with restart.auto off nothing restarts it on its own, so the line says only the key");
        assert_eq!(line(&stale("restarts when idle"), None, true).unwrap(), "↻ The lead runs older code than is installed · restarts when idle · L restarts it now");
    }

    #[test]
    fn a_refused_restart_shows_its_plain_reason_and_a_failure_stays_an_error() {
        let refused = outcome(Err("Not restarted: the lead is working.".to_string())).unwrap();
        assert!(refused.contains("Not restarted: the lead is working.") && refused.contains("L tries again") && !refused.contains("kelpie lead --restart"), "{refused}");
        assert_eq!(Notice::parse(&refused).map(|notice| notice.tone), Some(Tone::NotNow));
        assert_eq!(outcome(Err("Claude did not come back in pane p_9".to_string())), Err("Claude did not come back in pane p_9".to_string()));
        assert_eq!(outcome(Ok("Restarted the lead.".to_string())), Ok("Restarted the lead.".to_string()));
    }
}
