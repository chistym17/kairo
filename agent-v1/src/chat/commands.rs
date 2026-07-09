/// Parsed interactive input from stdin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatInput {
    Exit,
    Empty,
    Message(String),
}

pub fn parse_input(line: &str) -> ChatInput {
    let trimmed = line.trim();
    match trimmed {
        "" => ChatInput::Empty,
        "/exit" | "/quit" => ChatInput::Exit,
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
}
