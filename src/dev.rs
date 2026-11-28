// The one gate on kebbie's own development, kebbie_dev.fault_watcher or report.inbox_token in config.toml: every developer-only command, keeper routine, lead allow rule, role text and the maintainers' no-send asks here, and nothing else reads either key.
pub fn on() -> bool {
    crate::config::dev_machine().unwrap_or(false)
}

pub const COMMANDS: [&str; 2] = ["fault", "faults"];

pub fn hidden(command: &str) -> bool {
    COMMANDS.contains(&command) && !on()
}

pub fn shows_command(command: &str, dev: bool) -> bool {
    dev || !COMMANDS.contains(&command)
}

pub mod build;

#[cfg(test)]
mod tests;
