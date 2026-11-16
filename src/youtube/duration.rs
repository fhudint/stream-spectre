/// Parses the ISO 8601 durations the YouTube API returns (`PT1H2M3S`,
/// `P1DT2H`, `P0D`) into whole seconds.
///
/// Only days, hours, minutes and whole seconds are understood; anything else
/// (weeks, months, fractions, signs, lowercase, wrong order, overflow) yields
/// `None`.
pub fn parse_iso8601_duration(text: &str) -> Option<u64> {
    let rest = text.strip_prefix('P')?;
    let (date_part, time_part) = match rest.split_once('T') {
        Some((date, time)) => (date, Some(time)),
        None => (rest, None),
    };

    let mut total: u64 = 0;
    let mut components = 0;

    if !date_part.is_empty() {
        let days = parse_number(date_part.strip_suffix('D')?)?;
        total = total.checked_add(days.checked_mul(86_400)?)?;
        components += 1;
    }

    if let Some(time) = time_part {
        if time.is_empty() {
            return None;
        }
        let mut remaining = time;
        for (unit, seconds_per_unit) in [('H', 3_600u64), ('M', 60), ('S', 1)] {
            if let Some((number, after)) = remaining.split_once(unit) {
                let value = parse_number(number)?;
                total = total.checked_add(value.checked_mul(seconds_per_unit)?)?;
                remaining = after;
                components += 1;
            }
        }
        if !remaining.is_empty() {
            return None;
        }
    }

    (components > 0).then_some(total)
}

fn parse_number(digits: &str) -> Option<u64> {
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// `185` becomes `3:05`, `3723` becomes `1:02:03`.
pub fn format_duration(total_seconds: u64) -> String {
    let hours = total_seconds / 3_600;
    let minutes = (total_seconds % 3_600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_common_youtube_durations() {
        for (text, seconds) in [
            ("PT45S", 45),
            ("PT5M", 300),
            ("PT1H", 3_600),
            ("PT1H2M3S", 3_723),
            ("PT10M30S", 630),
            ("PT0S", 0),
            ("P0D", 0),
            ("P1D", 86_400),
            ("P1DT1H", 90_000),
            ("P1DT2H3M4S", 93_784),
            ("PT100S", 100),
            ("PT90M", 5_400),
        ] {
            assert_eq!(parse_iso8601_duration(text), Some(seconds), "{text}");
        }
    }

    #[test]
    fn rejects_malformed_or_unsupported_durations() {
        for text in [
            "",
            "P",
            "PT",
            "P1DT",
            "1H",
            "T1H",
            "PT1X",
            "PT-5S",
            "PT+5S",
            "PT1.5S",
            "PT1,5S",
            "PT1S2M", // wrong order
            "PT1H1H", // repeated unit
            "P1W",
            "P1Y",
            "P1M",  // months are not supported
            "pt5s", // lowercase
            " PT5S",
            "PT5S ",
            "PT 5S",
            "PTS",
            "PT٣S", // non-ASCII digit
            "P1DT1D",
            "PT99999999999999999999S", // does not fit in u64
            "P999999999999999999D",    // overflows when multiplied
        ] {
            assert_eq!(parse_iso8601_duration(text), None, "{text:?}");
        }
    }

    #[test]
    fn random_garbage_never_panics() {
        let alphabet: Vec<char> = "PTDHMS0123456789.-+ pt".chars().collect();
        let mut state = 99u64;
        for _ in 0..50_000 {
            let mut text = String::new();
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            let len = (state >> 60) as usize; // 0..=15
            for _ in 0..len {
                state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
                text.push(alphabet[((state >> 33) % alphabet.len() as u64) as usize]);
            }
            let _ = parse_iso8601_duration(&text);
        }
    }

    #[test]
    fn formats_durations_for_display() {
        assert_eq!(format_duration(0), "0:00");
        assert_eq!(format_duration(5), "0:05");
        assert_eq!(format_duration(185), "3:05");
        assert_eq!(format_duration(600), "10:00");
        assert_eq!(format_duration(3_723), "1:02:03");
        assert_eq!(format_duration(90_000), "25:00:00");
    }
}
