// The one gate on kebbie's own development, [kebbie_dev] in config.toml: every developer-only command, keeper routine, lead allow rule and role text asks here, and nothing else reads the flag.
pub fn on() -> bool {
    crate::config::fault_watcher_enabled().unwrap_or(false)
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
