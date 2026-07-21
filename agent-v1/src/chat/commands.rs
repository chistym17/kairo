/// Parsed interactive input from stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatInput {
    Exit,
    Empty,
    /// Show the durable session plan.
    ShowPlan,
    /// Show compact task state (objective, decisions, files, verification).
    ShowTask,
    /// Print a bounded repository overview (index-backed).
    Overview {
        depth: String,
        focus: Option<String>,
    },
    /// Mark the current plan Approved.
    ApprovePlan,
    /// Add a constraint to the current plan (returns to Draft).
    ConstrainPlan(String),
    /// Record a durable decision in task state.
    RecordDecision(String),
    /// Pin turn intent metadata (`explore` | `plan` | `execute` | `review`).
    SetIntent(crate::session::TurnIntent),
    /// Clear pinned intent (resume inference from messages).
    ClearIntent,
    Message(String),
}

pub fn parse_input(line: &str) -> ChatInput {
    let trimmed = line.trim();
    match trimmed {
        "" => ChatInput::Empty,
        "/exit" | "/quit" => ChatInput::Exit,
        "/plan" => ChatInput::ShowPlan,
        "/task" => ChatInput::ShowTask,
        "/overview" => ChatInput::Overview {
            depth: "overview".into(),
            focus: None,
        },
        "/approve-plan" => ChatInput::ApprovePlan,
        "/intent" => ChatInput::Message(trimmed.to_string()),
        "/intent clear" | "/intent auto" => ChatInput::ClearIntent,
        other if other.starts_with("/intent ") => {
            let name = other["/intent ".len()..].trim();
            match crate::session::TurnIntent::parse(name) {
                Some(intent) => ChatInput::SetIntent(intent),
                None => ChatInput::Message(other.to_string()),
            }
        }
        other if other.starts_with("/overview ") => {
            let rest = other["/overview ".len()..].trim();
            // `/overview structural` or `/overview structural tasks`
            let mut parts = rest.splitn(2, char::is_whitespace);
            let depth = parts.next().unwrap_or("overview").to_string();
            let focus = parts
                .next()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            ChatInput::Overview { depth, focus }
        }
        other if other.starts_with("/constrain ") => {
            let text = other["/constrain ".len()..].trim();
            if text.is_empty() {
                ChatInput::Message(other.to_string())
            } else {
                ChatInput::ConstrainPlan(text.to_string())
            }
        }
        other if other.starts_with("/decision ") => {
            let text = other["/decision ".len()..].trim();
            if text.is_empty() {
                ChatInput::Message(other.to_string())
            } else {
                ChatInput::RecordDecision(text.to_string())
            }
        }
        other if other == "/constrain" || other == "/decision" => {
            ChatInput::Message(other.to_string())
        }
        msg => ChatInput::Message(msg.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exit_commands() {
        assert_eq!(parse_input("/exit"), ChatInput::Exit);
        assert_eq!(parse_input("/quit"), ChatInput::Exit);
    }

    #[test]
    fn parses_empty_and_message() {
        assert_eq!(parse_input(""), ChatInput::Empty);
        assert_eq!(parse_input("  "), ChatInput::Empty);
        assert_eq!(
            parse_input("fix divide bug"),
            ChatInput::Message("fix divide bug".to_string())
        );
    }

    #[test]
    fn parses_plan_and_task_commands() {
        assert_eq!(parse_input("/plan"), ChatInput::ShowPlan);
        assert_eq!(parse_input("/task"), ChatInput::ShowTask);
        assert_eq!(
            parse_input("/overview"),
            ChatInput::Overview {
                depth: "overview".into(),
                focus: None
            }
        );
        assert_eq!(
            parse_input("/overview structural tasks"),
            ChatInput::Overview {
                depth: "structural".into(),
                focus: Some("tasks".into())
            }
        );
        assert_eq!(parse_input("/approve-plan"), ChatInput::ApprovePlan);
        assert_eq!(
            parse_input("/constrain frontend only"),
            ChatInput::ConstrainPlan("frontend only".to_string())
        );
        assert_eq!(
            parse_input("/decision skip backend"),
            ChatInput::RecordDecision("skip backend".to_string())
        );
        assert_eq!(
            parse_input("/intent plan"),
            ChatInput::SetIntent(crate::session::TurnIntent::Plan)
        );
        assert_eq!(parse_input("/intent clear"), ChatInput::ClearIntent);
    }
}
