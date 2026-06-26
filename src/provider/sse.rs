use crate::provider::ProviderError;
use std::io::{BufRead, BufReader, Read};

pub fn read_sse_data_lines<R, F>(reader: R, mut on_data: F) -> Result<(), ProviderError>
where
    R: Read,
    F: FnMut(&str) -> Result<(), ProviderError>,
{
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let bytes_read = reader
            .read_line(&mut line)
            .map_err(|error| ProviderError::new(format!("failed to read stream: {error}")))?;

        if bytes_read == 0 {
            break;
        }

        let line = line.trim_end_matches(['\r', '\n']);
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };

        let data = data.trim_start();
        if data == "[DONE]" {
            break;
        }

        if !data.is_empty() {
            on_data(data)?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_only_sse_data_lines() {
        let input = b"event: ping\n\ndata: {\"a\":1}\n\ndata: [DONE]\n";
        let mut events = Vec::new();

        read_sse_data_lines(&input[..], |data| {
            events.push(data.to_string());
            Ok(())
        })
        .unwrap();

        assert_eq!(events, vec![r#"{"a":1}"#]);
    }
}
