use crate::models::chat::ChatMessage;

/// Lightweight token estimate based on whitespace-delimited words.
pub fn estimate_tokens_for_text(text: &str) -> u32 {
    text.split_whitespace().count() as u32
}

/// Aggregate token estimate for all message contents.
pub fn estimate_tokens_for_messages(messages: &[ChatMessage]) -> u32 {
    messages
        .iter()
        .map(|message| estimate_tokens_for_text(&message.content))
        .sum()
}

#[cfg(test)]
mod tests {
    use crate::models::chat::{ChatMessage, ChatRole};

    use super::{estimate_tokens_for_messages, estimate_tokens_for_text};

    #[test]
    fn text_estimation_counts_whitespace_words() {
        assert_eq!(estimate_tokens_for_text("one two three"), 3);
        assert_eq!(estimate_tokens_for_text("  one   two  "), 2);
        assert_eq!(estimate_tokens_for_text(""), 0);
    }

    #[test]
    fn message_estimation_sums_message_contents() {
        let messages = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "hello there".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "general kenobi".into(),
            },
        ];

        assert_eq!(estimate_tokens_for_messages(&messages), 4);
    }
}
