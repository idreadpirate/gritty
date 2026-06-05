// Agent awareness: identify which AI coding agent runs in a pane and classify
// its live state (working / blocked / idle) from the bottom of the screen grid.
//
// Ported and condensed from herdr's `src/detect` (MIT). herdr is a unix
// client-server agent multiplexer that doesn't run on Windows; this brings its
// most valuable idea — a multiplexer that knows when your agent is blocked
// waiting for you, busy, or done — to gritty's local native-Windows model.
//
// The detector is pure (`&str -> AgentState`) and platform-agnostic. Identity
// comes from gritty's existing foreground-process name (`proc.rs`); state comes
// from the live screen tail (`Terminal::screen_tail`).

/// Which agent we recognize running in a pane. `None` ⇒ a plain shell/program.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Claude,
    Codex,
    Gemini,
    Cursor,
    Copilot,
    Pi,
    OpenCode,
    Droid,
    Amp,
    Aider,
    Grok,
    Qwen,
}

/// The detected state of an agent pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    #[default]
    Idle,
    /// Agent is actively working/processing.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
    /// Plain shell or unrecognized program — no agent in this pane.
    Unknown,
}

impl Agent {
    /// Short label shown in the pane header.
    pub fn label(self) -> &'static str {
        match self {
            Agent::Claude => "claude",
            Agent::Codex => "codex",
            Agent::Gemini => "gemini",
            Agent::Cursor => "cursor",
            Agent::Copilot => "copilot",
            Agent::Pi => "pi",
            Agent::OpenCode => "opencode",
            Agent::Droid => "droid",
            Agent::Amp => "amp",
            Agent::Aider => "aider",
            Agent::Grok => "grok",
            Agent::Qwen => "qwen",
        }
    }
}

/// Identify the agent from a foreground process name (already `.exe`-stripped by
/// `proc.rs`). Case-insensitive; returns `None` for plain shells/programs.
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    let name = process_name.trim().to_ascii_lowercase();
    let name = name.strip_suffix(".exe").unwrap_or(&name);
    match name {
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        "gemini" => Some(Agent::Gemini),
        "cursor" | "cursor-agent" => Some(Agent::Cursor),
        "copilot" | "github-copilot" | "ghcs" => Some(Agent::Copilot),
        "pi" => Some(Agent::Pi),
        "opencode" | "open-code" => Some(Agent::OpenCode),
        "droid" => Some(Agent::Droid),
        "amp" | "amp-local" => Some(Agent::Amp),
        "aider" => Some(Agent::Aider),
        "grok" | "grok-build" => Some(Agent::Grok),
        "qwen" | "qwen-code" => Some(Agent::Qwen),
        _ => None,
    }
}

/// Detect agent state from the live terminal tail. `None` agent ⇒ `Unknown`.
pub fn detect_state(agent: Option<Agent>, screen: &str) -> AgentState {
    let Some(agent) = agent else {
        return AgentState::Unknown;
    };
    match agent {
        Agent::Claude => detect_claude(screen),
        _ => detect_generic(screen),
    }
}

/// Whether a state change from `old` to `new` warrants notifying the user: the
/// agent was actively working and has now either finished (→ Idle) or stopped to
/// ask for input (→ Blocked). Conservative on purpose — transitions that don't
/// start from `Working` (e.g. the first classification of a freshly-spawned
/// agent, or routine idle↔idle) never fire, so a pane you aren't watching only
/// pings you when it actually needs you.
pub fn is_attention_transition(old: AgentState, new: AgentState) -> bool {
    use AgentState::*;
    matches!((old, new), (Working, Idle) | (Working, Blocked))
}

/// Color-neutral status glyph (CA-25) for a pane's agent, shared by the header
/// and the overview panel. A raised attention latch wins over the live state.
pub fn state_badge(state: AgentState, attention: bool) -> &'static str {
    if attention {
        return "★"; // finished/blocked while unwatched — look here
    }
    match state {
        AgentState::Working => "●", // busy
        AgentState::Blocked => "◆", // needs input
        _ => "○",                   // idle / done / unknown
    }
}

// ---------------------------------------------------------------------------
// Shared heuristics
// ---------------------------------------------------------------------------

/// A line-leading braille spinner glyph (U+2800..=U+28FF) used by most CLI
/// spinners ⇒ the agent is actively rendering progress.
fn has_braille_spinner(content: &str) -> bool {
    content.lines().any(|line| {
        line.trim()
            .chars()
            .next()
            .is_some_and(|c| ('\u{2800}'..='\u{28FF}').contains(&c))
    })
}

/// A live "press X to interrupt/cancel/stop" hint ⇒ the agent is working.
fn has_interrupt_hint(lower: &str) -> bool {
    lower.contains("esc to interrupt")
        || lower.contains("ctrl+c to interrupt")
        || lower.contains("press esc to interrupt")
        || lower.contains("esc to cancel")
        || lower.contains("ctrl+c to stop")
        || lower.contains("press esc to stop")
        || lower.contains("esc to stop")
        || lower.contains("esc cancel")
}

/// A "Do you want to…/Would you like to…" confirmation followed by a yes/❯.
fn has_confirmation_prompt(lower: &str) -> bool {
    if let Some(pos) = lower
        .find("do you want to")
        .or_else(|| lower.find("would you like to"))
    {
        let after = &lower[pos..];
        return after.contains("yes") || after.contains('❯');
    }
    false
}

/// A "❯" selection cursor on a numbered option (`❯ 1. …`) ⇒ a blocking menu.
fn has_selection_prompt(content: &str) -> bool {
    content.lines().any(|line| {
        let t = line.trim();
        t.starts_with('❯') && t.chars().any(|c| c.is_ascii_digit()) && t.contains('.')
    })
}

/// Generic detector for agents without a bespoke screen layout. Covers the
/// common cases: an interrupt hint or spinner ⇒ Working; a confirmation or
/// selection prompt ⇒ Blocked; otherwise Idle.
fn detect_generic(content: &str) -> AgentState {
    let lower = content.to_ascii_lowercase();
    if has_confirmation_prompt(&lower) || has_selection_prompt(content) {
        return AgentState::Blocked;
    }
    if has_interrupt_hint(&lower) || has_braille_spinner(content) {
        return AgentState::Working;
    }
    AgentState::Idle
}

// ---------------------------------------------------------------------------
// Claude Code — has a structured prompt-box UI, so it gets precise handling.
//
//   (agent output / tool results)
//   ───────────────────────────── (top border)
//   ❯ _                            (prompt line)
//   ───────────────────────────── (bottom border)
// ---------------------------------------------------------------------------

fn detect_claude(content: &str) -> AgentState {
    let lower = content.to_ascii_lowercase();

    // A live search box is always idle (user is browsing, not blocked).
    if content.contains("⌕ Search…") {
        return AgentState::Idle;
    }

    // A live blocking form below the last rule (question/permission menu).
    if claude_has_live_blocked_form(content) {
        return AgentState::Blocked;
    }

    // Working chrome (spinner or interrupt hint) ABOVE the prompt box.
    if claude_has_working_chrome(content) {
        return AgentState::Working;
    }

    // A blocking confirmation only counts when there is no live prompt box —
    // an old permission prompt scrolled above a fresh prompt box is idle.
    if !claude_has_prompt_box(content) && claude_has_blocked_prompt(content, &lower) {
        return AgentState::Blocked;
    }

    AgentState::Idle
}

fn claude_has_blocked_prompt(content: &str, lower: &str) -> bool {
    has_confirmation_prompt(lower)
        || lower.contains("do you want to proceed?")
        || lower.contains("would you like to proceed?")
        || lower.contains("waiting for permission")
        || lower.contains("tab to amend")
        || lower.contains("ctrl+e to explain")
        || (has_selection_prompt(content) && claude_has_yes_no_choice(content))
}

/// A live question/permission form below the last horizontal rule, recognized
/// by its footer (`Enter to select … Esc to cancel … navigate`).
fn claude_has_live_blocked_form(content: &str) -> bool {
    content_after_last_rule(content).lines().any(|line| {
        let l = line.to_ascii_lowercase();
        l.contains("enter to select")
            && l.contains("esc to cancel")
            && (l.contains("to navigate") || l.contains("↑/↓") || l.contains("↑↓"))
    })
}

fn claude_has_working_chrome(content: &str) -> bool {
    let above = content_above_prompt_box(content).to_ascii_lowercase();
    above.contains("esc to interrupt")
        || above.contains("ctrl+c to interrupt")
        || claude_has_spinner_activity(&above)
}

/// Claude's spinner glyph + activity label (verb changes constantly, so match
/// the glyph + trailing ellipsis, not the wording).
fn claude_has_spinner_activity(lower_content: &str) -> bool {
    const SPINNER: &str = "·✱✲✳✴✵✶✷✸✹✺✻✼✽✾✿❀❁❂❃❇❈❉❊❋✢✣✤✥✦✧✨⊛⊕⊙◉◎◍";
    lower_content.lines().any(|line| {
        let t = line.trim();
        let mut chars = t.chars();
        match chars.next() {
            // glyph, then a space, then a label ending in an ellipsis (…).
            Some(first) if SPINNER.contains(first) && chars.next() == Some(' ') => {
                let rest = chars.as_str();
                rest.contains('\u{2026}') && rest.chars().any(|c| c.is_alphanumeric())
            }
            _ => false,
        }
    })
}

fn claude_has_yes_no_choice(content: &str) -> bool {
    content.lines().any(|line| {
        let t = line
            .trim()
            .trim_start_matches('❯')
            .trim_start()
            .to_ascii_lowercase();
        t == "yes" || t == "no" || t.starts_with("1. yes") || t.starts_with("2. no")
    })
}

/// Is there a live prompt box (two ─ borders with a `❯` line between them)?
fn claude_has_prompt_box(content: &str) -> bool {
    let Some(top) = claude_prompt_box_top(content) else {
        return false;
    };
    content[top..]
        .lines()
        .skip(1) // the border line itself
        .take_while(|l| !is_horizontal_rule(l))
        .any(|l| l.trim_start().starts_with('❯'))
}

fn content_above_prompt_box(content: &str) -> &str {
    claude_prompt_box_top(content).map_or(content, |off| &content[..off.min(content.len())])
}

fn content_after_last_rule(content: &str) -> &str {
    let mut last_end = 0usize;
    let mut off = 0usize;
    for line in content.lines() {
        let next = off + line.len() + 1;
        if is_horizontal_rule(line) {
            last_end = next.min(content.len());
        }
        off = next;
    }
    &content[last_end..]
}

/// Byte offset where the prompt box's top border begins: the 2nd horizontal
/// rule counting from the bottom (the box is bordered above and below the `❯`
/// line). Single forward pass tracking the last two rule offsets — no `Vec`.
fn claude_prompt_box_top(content: &str) -> Option<usize> {
    let (mut last, mut second_last) = (None, None);
    let mut off = 0usize;
    for line in content.lines() {
        if is_horizontal_rule(line) {
            second_last = last;
            last = Some(off);
        }
        off += line.len() + 1;
    }
    second_last
}

fn is_horizontal_rule(line: &str) -> bool {
    let t = line.trim();
    if t.is_empty() {
        return false;
    }
    let rule_chars = t.chars().take_while(|&c| c == '─').count();
    if rule_chars == 0 {
        return false;
    }
    let rule_bytes = t
        .char_indices()
        .nth(rule_chars)
        .map(|(i, _)| i)
        .unwrap_or(t.len());
    let suffix = t[rule_bytes..].trim_start();
    suffix.is_empty() || rule_chars >= 3
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude(s: &str) -> AgentState {
        detect_state(Some(Agent::Claude), s)
    }

    // ---- identification ----

    #[test]
    fn identifies_known_agents() {
        assert_eq!(identify_agent("claude"), Some(Agent::Claude));
        assert_eq!(identify_agent("claude-code"), Some(Agent::Claude));
        assert_eq!(identify_agent("CLAUDE"), Some(Agent::Claude));
        assert_eq!(identify_agent("codex"), Some(Agent::Codex));
        assert_eq!(identify_agent("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(identify_agent("ghcs"), Some(Agent::Copilot));
        assert_eq!(identify_agent("opencode.exe"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("aider"), Some(Agent::Aider));
    }

    #[test]
    fn ignores_plain_programs() {
        assert_eq!(identify_agent("pwsh"), None);
        assert_eq!(identify_agent("bash"), None);
        assert_eq!(identify_agent("vim"), None);
        assert_eq!(identify_agent("node"), None);
    }

    #[test]
    fn no_agent_is_unknown() {
        assert_eq!(detect_state(None, "anything"), AgentState::Unknown);
    }

    // ---- Claude: working ----

    #[test]
    fn claude_working_esc_to_interrupt() {
        let s = "Reading file src/main.rs\nesc to interrupt\n─────────\n❯ \n─────────";
        assert_eq!(claude(s), AgentState::Working);
    }

    #[test]
    fn claude_working_spinner() {
        assert_eq!(
            claude("✽ Tempering…\n─────────\n❯ \n─────────"),
            AgentState::Working
        );
    }

    #[test]
    fn claude_working_spinner_above_prompt_not_confused_by_box() {
        let s = "✽ Writing…\nesc to interrupt\n──────\n❯ \n──────";
        assert_eq!(claude(s), AgentState::Working);
    }

    // ---- Claude: blocked ----

    #[test]
    fn claude_blocked_do_you_want() {
        assert_eq!(
            claude("Do you want to run this command?\n\nYes  No"),
            AgentState::Blocked
        );
    }

    #[test]
    fn claude_blocked_permission_menu() {
        let s = "Do you want to proceed?\n❯ 1. Yes\n  2. No\n\nEsc to cancel · Tab to amend";
        assert_eq!(claude(s), AgentState::Blocked);
    }

    #[test]
    fn claude_blocked_question_form() {
        let s = "Which approach?\n❯ 1. Minimal\n  2. Refactor\n\nEnter to select · Tab/Arrow keys to navigate · Esc to cancel";
        assert_eq!(claude(s), AgentState::Blocked);
    }

    // ---- Claude: idle ----

    #[test]
    fn claude_idle_prompt_box() {
        assert_eq!(
            claude("Task complete.\n─────────────\n❯ \n─────────────"),
            AgentState::Idle
        );
    }

    #[test]
    fn claude_idle_search() {
        assert_eq!(claude("⌕ Search…\nsome content"), AgentState::Idle);
    }

    #[test]
    fn claude_old_permission_above_live_prompt_box_is_idle() {
        let s = "● Bash(rm -rf /tmp/test)\n  ⎿  Waiting…\n\nDo you want to proceed?\n❯ 1. Yes\n  2. No\n\nEsc to cancel · Tab to amend\n\n─────────────────────────────\n❯ \n─────────────────────────────\n  ~/P ⎇ master";
        assert_eq!(claude(s), AgentState::Idle);
    }

    // ---- generic agents ----

    #[test]
    fn generic_working_interrupt_hint() {
        assert_eq!(
            detect_state(Some(Agent::Codex), "generating\nesc to interrupt"),
            AgentState::Working
        );
        assert_eq!(
            detect_state(Some(Agent::Gemini), "thinking\nEsc to cancel"),
            AgentState::Working
        );
        assert_eq!(
            detect_state(Some(Agent::Cursor), "working\nCtrl+C to stop"),
            AgentState::Working
        );
    }

    #[test]
    fn generic_working_braille_spinner() {
        assert_eq!(
            detect_state(Some(Agent::Codex), "⠋ Thinking..."),
            AgentState::Working
        );
        assert_eq!(
            detect_state(Some(Agent::Droid), "⠹ Press ESC to stop"),
            AgentState::Working
        );
    }

    #[test]
    fn generic_blocked_confirmation() {
        assert_eq!(
            detect_state(Some(Agent::Codex), "Do you want to apply this patch? ❯ Yes"),
            AgentState::Blocked
        );
    }

    #[test]
    fn generic_idle_bare_prompt() {
        assert_eq!(detect_state(Some(Agent::Codex), "› "), AgentState::Idle);
        assert_eq!(
            detect_state(Some(Agent::Gemini), "ready\n> "),
            AgentState::Idle
        );
    }

    // ---- attention transitions ----

    #[test]
    fn attention_fires_when_work_finishes_or_blocks() {
        use AgentState::*;
        assert!(is_attention_transition(Working, Idle)); // finished
        assert!(is_attention_transition(Working, Blocked)); // needs input
    }

    #[test]
    fn attention_silent_on_non_working_origins() {
        use AgentState::*;
        // A freshly-spawned agent's first classification must not ping.
        assert!(!is_attention_transition(Unknown, Idle));
        assert!(!is_attention_transition(Unknown, Working));
        // Non-Working origins stay quiet.
        assert!(!is_attention_transition(Idle, Blocked));
        assert!(!is_attention_transition(Idle, Working));
        assert!(!is_attention_transition(Blocked, Working));
        assert!(!is_attention_transition(Idle, Idle));
    }

    #[test]
    fn badge_attention_overrides_state() {
        use AgentState::*;
        assert_eq!(state_badge(Working, true), "★");
        assert_eq!(state_badge(Idle, true), "★");
        assert_eq!(state_badge(Working, false), "●");
        assert_eq!(state_badge(Blocked, false), "◆");
        assert_eq!(state_badge(Idle, false), "○");
    }
}
