use std::collections::BTreeSet;

/// Detect likely listening ports from terminal text snapshots.
/// Matches `:<port>` patterns and returns unique sorted values.
pub fn detect_ports_in_text(text: &str) -> Vec<u16> {
    let bytes = text.as_bytes();
    let mut ports = BTreeSet::new();
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] != b':' {
            i += 1;
            continue;
        }

        let mut j = i + 1;
        while j < bytes.len() && bytes[j].is_ascii_digit() && (j - (i + 1)) < 5 {
            j += 1;
        }
        if j == i + 1 {
            i += 1;
            continue;
        }

        // Ensure boundary after the number.
        if j < bytes.len() && bytes[j].is_ascii_digit() {
            i += 1;
            continue;
        }

        if let Ok(raw) = std::str::from_utf8(&bytes[i + 1..j]) {
            if let Ok(port) = raw.parse::<u16>() {
                if port > 0 {
                    ports.insert(port);
                }
            }
        }
        i = j;
    }

    ports.into_iter().collect()
}

