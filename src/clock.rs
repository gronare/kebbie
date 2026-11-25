pub fn fix() -> &'static str {
    if cfg!(target_os = "macos") { "sudo sntp -sS time.apple.com" } else { "sudo chronyc makestep" }
}

// A command signed further ahead than the protocol's skew allows was refused only because this machine's clock is behind.
pub fn refused(sent_at: u64, now: u64) -> Option<String> {
    (sent_at > now + kebbie_hub_protocol::CLOCK_SKEW).then(|| format!("Not done: the machine's clock runs {} behind, so this looked expired. Set its clock, for example with {}, then send it again.", crate::keeper::span(sent_at - now), fix()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_from_hours_ahead_says_the_machines_clock_is_behind_and_how_to_set_it() {
        let now = 1_795_607_054;
        let said = refused(now + 23_316, now).expect("6.5 hours ahead is the machine's clock");
        assert!(said.contains("clock runs 6h 28m behind") && said.contains(fix()) && said.ends_with("then send it again."), "{said}");
        assert_eq!(refused(now + kebbie_hub_protocol::CLOCK_SKEW, now), None, "within the protocol's skew the command really expired");
        assert_eq!(refused(now - 3600, now), None);
    }
}
