use crate::providers::provider::ProviderError;

const MAX_SSE_BUFFER_BYTES: usize = 256 * 1024;
const MAX_SSE_EVENT_BYTES: usize = 256 * 1024;

#[derive(Default)]
pub struct SseDataParser {
    buffer: String,
}

impl SseDataParser {
    pub fn push_chunk(&mut self, chunk: &str) -> Result<Vec<String>, ProviderError> {
        if self.buffer.len().saturating_add(chunk.len()) > MAX_SSE_BUFFER_BYTES {
            return Err(ProviderError::ProviderBadResponse);
        }

        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(delimiter_index) = self.buffer.find("\n\n") {
            if delimiter_index > MAX_SSE_EVENT_BYTES {
                return Err(ProviderError::ProviderBadResponse);
            }
            let raw_event = self.buffer[..delimiter_index].to_string();
            self.buffer.drain(..delimiter_index + 2);
            if let Some(event_data) = parse_event_data(&raw_event) {
                if event_data.len() > MAX_SSE_EVENT_BYTES {
                    return Err(ProviderError::ProviderBadResponse);
                }
                events.push(event_data);
            }
        }

        Ok(events)
    }
}

fn parse_event_data(raw_event: &str) -> Option<String> {
    let data_lines: Vec<&str> = raw_event
        .lines()
        .filter_map(|line| line.strip_prefix("data:"))
        .map(str::trim_start)
        .filter(|line| !line.is_empty())
        .collect();

    if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::SseDataParser;

    #[test]
    fn parser_collects_events_across_partial_chunks() {
        let mut parser = SseDataParser::default();
        let events = parser.push_chunk("data: {\"a\":1}\n\nda").unwrap();
        assert_eq!(events, vec!["{\"a\":1}".to_string()]);

        let events = parser.push_chunk("ta: {\"b\":2}\n\n").unwrap();
        assert_eq!(events, vec!["{\"b\":2}".to_string()]);
    }
}
