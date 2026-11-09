use std::path::Path;

use serde_json::{Value, json};

// What Claude Code or Codex sends a hook on stdin; input that is not JSON reads as an empty payload, so every field is absent.
pub struct Payload(Value);

impl Payload {
    pub fn parse(input: &str) -> Payload {
        Payload(serde_json::from_str(input).unwrap_or_default())
    }

    pub fn value(&self) -> &Value {
        &self.0
    }

    pub fn event(&self) -> Option<&str> {
        self.0["hook_event_name"].as_str()
    }

    pub fn tool_name(&self) -> Option<&str> {
        self.0["tool_name"].as_str()
    }

    pub fn tool_input(&self) -> &Value {
        &self.0["tool_input"]
    }

    pub fn command(&self) -> Option<&str> {
        self.0["tool_input"]["command"].as_str()
    }

    pub fn cwd(&self) -> Option<&Path> {
        self.0["cwd"].as_str().map(Path::new)
    }

    pub fn prompt(&self) -> Option<&str> {
        self.0["prompt"].as_str()
    }

    pub fn stop_hook_active(&self) -> bool {
        self.0["stop_hook_active"] == true
    }
}

// A hook's answer on stdout, in the shape Claude Code and Codex both read.
#[derive(Debug, PartialEq)]
pub enum Answer {
    Silent,
    Deny(String),
    Rewrite(Value),
    // The event is echoed as the payload named it, absent or not.
    Context { event: Value, text: String },
    Warn(String),
    Block(String),
    Text(String),
}

impl Answer {
    pub fn deny_if(reason: Option<String>) -> Answer {
        reason.map_or(Answer::Silent, Answer::Deny)
    }

    pub fn render(&self) -> String {
        match self {
            Answer::Silent => String::new(),
            Answer::Deny(reason) => json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "permissionDecision": "deny", "permissionDecisionReason": reason } }).to_string(),
            Answer::Rewrite(tool_input) => json!({ "hookSpecificOutput": { "hookEventName": "PreToolUse", "updatedInput": tool_input } }).to_string(),
            Answer::Context { event, text } => json!({ "hookSpecificOutput": { "hookEventName": event, "additionalContext": text } }).to_string(),
            Answer::Warn(text) => json!({ "systemMessage": text }).to_string(),
            Answer::Block(reason) => json!({ "decision": "block", "reason": reason }).to_string(),
            Answer::Text(text) => text.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_answer_renders_the_bytes_the_hooks_always_printed() {
        assert_eq!(Answer::Silent.render(), "");
        assert_eq!(Answer::deny_if(None), Answer::Silent);
        assert_eq!(Answer::Deny("no \"push\"".to_string()).render(), r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"no \"push\""}}"#);
        assert_eq!(Answer::deny_if(Some("why".to_string())).render(), r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"why"}}"#);
        assert_eq!(Answer::Rewrite(json!({ "command": "git diff --no-ext-diff", "description": "read" })).render(), r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"git diff --no-ext-diff","description":"read"}}}"#);
        assert_eq!(Answer::Context { event: json!("PostToolUse"), text: "a\nb".to_string() }.render(), r#"{"hookSpecificOutput":{"additionalContext":"a\nb","hookEventName":"PostToolUse"}}"#);
        assert_eq!(Answer::Context { event: Value::Null, text: "x".to_string() }.render(), r#"{"hookSpecificOutput":{"additionalContext":"x","hookEventName":null}}"#);
        assert_eq!(Answer::Warn("down".to_string()).render(), r#"{"systemMessage":"down"}"#);
        assert_eq!(Answer::Block("report first".to_string()).render(), r#"{"decision":"block","reason":"report first"}"#);
        assert_eq!(Answer::Text("stale\n".to_string()).render(), "stale\n");
    }

    #[test]
    fn a_payload_reads_the_fields_the_hooks_use_and_nothing_from_input_that_is_not_json() {
        let payload = Payload::parse(&json!({ "hook_event_name": "PreToolUse", "tool_name": "Bash", "tool_input": { "command": "ls" }, "cwd": "/src/app", "prompt": "go", "stop_hook_active": true }).to_string());
        assert_eq!((payload.event(), payload.tool_name(), payload.command(), payload.cwd(), payload.prompt(), payload.stop_hook_active()), (Some("PreToolUse"), Some("Bash"), Some("ls"), Some(Path::new("/src/app")), Some("go"), true));
        assert_eq!(payload.tool_input(), &json!({ "command": "ls" }));
        let broken = Payload::parse("not json");
        assert_eq!((broken.event(), broken.tool_name(), broken.command(), broken.cwd(), broken.stop_hook_active()), (None, None, None, None, false));
        assert_eq!(broken.value(), &Value::Null);
        assert!(!Payload::parse(r#"{"stop_hook_active":"true"}"#).stop_hook_active(), "only the JSON true counts");
    }
}
