/// The agents omarchy knows how to launch, with the same "don't stop to ask"
/// flags that `bin/omarchy-agent` uses for each one.
pub const AGENTS: [&str; 9] = [
    "claude", "codex", "opencode", "gemini", "copilot", "crush", "grok", "omp", "pi",
];

pub fn command(agent: &str) -> Option<&'static str> {
    Some(match agent {
        "claude" => "claude --permission-mode auto",
        "codex" => "codex --approve-for-me",
        "opencode" => "opencode --auto",
        "gemini" => "gemini --yolo",
        "copilot" => "copilot --allow-all",
        "crush" => "crush --yolo",
        "grok" => "grok --permission-mode bypassPermissions",
        "omp" => "omp --auto-approve",
        "pi" => "pi",
        _ => return None,
    })
}

/// Single-quote for `sh -c`, since the command string is handed to tmux.
fn quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

/// The same per-agent prompt flags omarchy uses, so a workspace can start with
/// its task already given instead of waiting at an empty prompt.
pub fn command_with_prompt(agent: &str, prompt: &str) -> Option<String> {
    let base = command(agent)?;
    if prompt.trim().is_empty() {
        return Some(base.to_string());
    }
    let prompt = quote(prompt);
    Some(match agent {
        "opencode" => format!("{base} --prompt {prompt}"),
        "gemini" => format!("{base} --prompt-interactive {prompt}"),
        "copilot" => format!("{base} --interactive {prompt}"),
        // --yolo belongs to interactive crush only; `crush run` never prompts.
        "crush" => format!("crush run {prompt}"),
        "pi" => format!("{base} {prompt}"),
        "claude" | "codex" | "grok" | "omp" => format!("{base} -- {prompt}"),
        _ => return None,
    })
}

fn on_path(binary: &str) -> bool {
    let Ok(path) = std::env::var("PATH") else {
        return false;
    };
    path.split(':')
        .any(|dir| std::path::Path::new(dir).join(binary).is_file())
}

pub fn installed() -> Vec<String> {
    AGENTS
        .iter()
        .filter(|a| on_path(a))
        .map(|a| a.to_string())
        .collect()
}

/// Whatever `omarchy default agent` is set to, falling back to the first
/// installed agent so the picker always opens on something usable.
pub fn default_agent() -> String {
    if let Ok(out) = std::process::Command::new("omarchy-default-agent").output() {
        let name = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !name.is_empty() {
            return name;
        }
    }
    installed()
        .first()
        .cloned()
        .unwrap_or_else(|| "claude".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_task_leaves_the_command_alone() {
        assert_eq!(
            command_with_prompt("claude", "   ").unwrap(),
            "claude --permission-mode auto"
        );
    }

    #[test]
    fn each_agent_gets_its_own_prompt_flag() {
        assert_eq!(
            command_with_prompt("claude", "hi").unwrap(),
            "claude --permission-mode auto -- 'hi'"
        );
        assert_eq!(
            command_with_prompt("codex", "hi").unwrap(),
            "codex --approve-for-me -- 'hi'"
        );
        assert_eq!(
            command_with_prompt("opencode", "hi").unwrap(),
            "opencode --auto --prompt 'hi'"
        );
        assert_eq!(
            command_with_prompt("gemini", "hi").unwrap(),
            "gemini --yolo --prompt-interactive 'hi'"
        );
        assert_eq!(
            command_with_prompt("copilot", "hi").unwrap(),
            "copilot --allow-all --interactive 'hi'"
        );
        assert_eq!(command_with_prompt("pi", "hi").unwrap(), "pi 'hi'");
    }

    #[test]
    fn crush_uses_run_because_yolo_is_interactive_only() {
        assert_eq!(
            command_with_prompt("crush", "hi").unwrap(),
            "crush run 'hi'"
        );
    }

    #[test]
    fn a_quote_in_the_task_cannot_break_out_of_the_command() {
        assert_eq!(
            command_with_prompt("claude", "don't $(rm -rf /) break").unwrap(),
            r"claude --permission-mode auto -- 'don'\''t $(rm -rf /) break'"
        );
    }

    #[test]
    fn an_unknown_agent_has_no_command() {
        assert!(command_with_prompt("nope", "hi").is_none());
    }
}
