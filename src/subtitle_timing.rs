//! Narrow, bounded subtitle cue-window processing for the pinned Jellyfin
//! HLS text-subtitle transport. This does not attempt general WebVTT parsing.

/// Decode a bounded text subtitle without assuming that SRT files use
/// UTF-8. Explicit UTF-16 BOMs are supported; unmarked invalid UTF-8 fails
/// closed rather than corrupting international captions.
pub(crate) fn decode_text_subtitle(input: &[u8]) -> Option<String> {
    if input.len() > 4 * 1024 * 1024 {
        return None;
    }
    let (data, little_endian) = if let Some(rest) = input.strip_prefix(&[0xff, 0xfe]) {
        (rest, true)
    } else if let Some(rest) = input.strip_prefix(&[0xfe, 0xff]) {
        (rest, false)
    } else {
        return Some(
            std::str::from_utf8(input)
                .ok()?
                .trim_start_matches('\u{feff}')
                .to_owned(),
        );
    };
    if data.len() % 2 != 0 {
        return None;
    }
    let units = data.chunks_exact(2).map(|bytes| {
        if little_endian {
            u16::from_le_bytes([bytes[0], bytes[1]])
        } else {
            u16::from_be_bytes([bytes[0], bytes[1]])
        }
    });
    std::char::decode_utf16(units).collect::<Result<String, _>>().ok()
}

fn parse_stamp(stamp: &str) -> Option<u64> {
    let (clock, fraction) = stamp.split_once('.')?;
    if fraction.len() != 3 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let mut fields = clock.split(':');
    let hours = fields.next()?;
    let minutes = fields.next()?;
    let seconds = fields.next()?;
    if fields.next().is_some()
        || !(2..=3).contains(&hours.len())
        || minutes.len() != 2
        || seconds.len() != 2
        || ![hours, minutes, seconds]
            .iter()
            .all(|field| field.bytes().all(|digit| digit.is_ascii_digit()))
    {
        return None;
    }
    let h = hours.parse::<u64>().ok()?;
    let m = minutes.parse::<u64>().ok()?;
    let s = seconds.parse::<u64>().ok()?;
    if m >= 60 || s >= 60 {
        return None;
    }
    h.checked_mul(3600)?
        .checked_add(m.checked_mul(60)?)?
        .checked_add(s)?
        .checked_mul(10_000_000)?
        .checked_add(fraction.parse::<u64>().ok()?.checked_mul(10_000)?)
}

fn write_stamp(ticks: u64) -> String {
    let milliseconds = ticks / 10_000;
    let hours = milliseconds / 3_600_000;
    let minutes = (milliseconds / 60_000) % 60;
    let seconds = (milliseconds / 1_000) % 60;
    let remainder = milliseconds % 1_000;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{remainder:03}")
}

/// Clip simple cue ranges into a short VOD window. Clamp overlapping cues,
/// preserving source-clock timestamps only when explicitly requested.
/// An empty output is valid for a subtitle-free window.
pub(crate) fn clip_vtt(
    source: &[u8],
    start_ticks: u64,
    end_ticks: u64,
    copy_timestamps: bool,
) -> Option<Vec<u8>> {
    if end_ticks <= start_ticks || end_ticks - start_ticks > 600_000_000 {
        return None;
    }
    let source = std::str::from_utf8(source)
        .ok()?
        .trim_start_matches('\u{feff}');
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let body = normalized.strip_prefix("WEBVTT\n\n")?;
    let mut result = String::from("WEBVTT\n\n");
    let mut cues = 0_u32;
    for block in body.split("\n\n").filter(|part| !part.trim().is_empty()) {
        let mut lines = block.lines();
        let first = lines.next()?.trim();
        let timing = if first.contains(" --> ") {
            first
        } else if first.bytes().all(|byte| byte.is_ascii_digit()) && !first.is_empty() {
            lines.next()?.trim()
        } else {
            return None;
        };
        let (start, end) = timing.split_once(" --> ")?;
        let cue_start = parse_stamp(start)?;
        let cue_end = parse_stamp(end)?;
        if cue_end <= cue_start {
            return None;
        }
        let caption = lines.collect::<Vec<_>>().join("\n");
        if caption.trim().is_empty() {
            return None;
        }
        let selected_start = cue_start.max(start_ticks);
        let selected_end = cue_end.min(end_ticks);
        if selected_start < selected_end {
            let offset = if copy_timestamps { 0 } else { start_ticks };
            result.push_str(&write_stamp(selected_start - offset));
            result.push_str(" --> ");
            result.push_str(&write_stamp(selected_end - offset));
            result.push('\n');
            result.push_str(caption.trim_end());
            result.push_str("\n\n");
        }
        cues += 1;
        if cues > 100_000 || result.len() > 4 * 1024 * 1024 + 4096 {
            return None;
        }
    }
    Some(result.into_bytes())
}

/// The bounded SRT emitter supports only simple cues produced by clip_vtt.
pub(crate) fn vtt_to_srt(source: &[u8]) -> Option<Vec<u8>> {
    let source = std::str::from_utf8(source).ok()?;
    let body = source.strip_prefix("WEBVTT\n\n")?;
    let mut result = String::new();
    for (number, block) in body
        .split("\n\n")
        .filter(|part| !part.trim().is_empty())
        .enumerate()
    {
        let (timing, caption) = block.split_once('\n')?;
        let (start, end) = timing.split_once(" --> ")?;
        if parse_stamp(start).is_none() || parse_stamp(end).is_none() {
            return None;
        }
        result.push_str(&(number + 1).to_string());
        result.push('\n');
        result.push_str(&start.replace('.', ","));
        result.push_str(" --> ");
        result.push_str(&end.replace('.', ","));
        result.push('\n');
        result.push_str(caption);
        result.push_str("\n\n");
        if result.len() > 4 * 1024 * 1024 + 4096 {
            return None;
        }
    }
    Some(result.into_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srt_decoding_accepts_bom_utf16_without_replacing_invalid_data() {
        let caption = "1\n00:00:00,000 --> 00:00:01,000\nOlá\n";
        let mut utf16le = vec![0xff, 0xfe];
        utf16le.extend(caption.encode_utf16().flat_map(u16::to_le_bytes));
        let mut utf16be = vec![0xfe, 0xff];
        utf16be.extend(caption.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_text_subtitle(&utf16le).as_deref(), Some(caption));
        assert_eq!(decode_text_subtitle(&utf16be).as_deref(), Some(caption));
        assert_eq!(decode_text_subtitle(caption.as_bytes()).as_deref(), Some(caption));
        assert!(decode_text_subtitle(b"\xff\xfe\xff").is_none());
        assert!(decode_text_subtitle(&[0xff, 0xff]).is_none());
        assert!(decode_text_subtitle(&[0xff, 0xfe, 0x00, 0xd8]).is_none());
    }

    #[test]
    fn clip_clamps_cues_and_supports_absolute_or_rebased_timestamps() {
        let vtt = b"WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nFirst\n\n00:00:04.000 --> 00:00:06.000\nSecond\n\n";
        let relative = clip_vtt(vtt, 20_000_000, 50_000_000, false).unwrap();
        assert_eq!(
            relative,
            b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000\nFirst\n\n00:00:02.000 --> 00:00:03.000\nSecond\n\n"
        );
        let absolute = clip_vtt(vtt, 20_000_000, 50_000_000, true).unwrap();
        assert_eq!(
            absolute,
            b"WEBVTT\n\n00:00:02.000 --> 00:00:03.000\nFirst\n\n00:00:04.000 --> 00:00:05.000\nSecond\n\n"
        );
        assert_eq!(
            vtt_to_srt(&relative).unwrap(),
            b"1\n00:00:00,000 --> 00:00:01,000\nFirst\n\n2\n00:00:02,000 --> 00:00:03,000\nSecond\n\n"
        );
    }

    #[test]
    fn empty_window_and_malformed_timing_fail_closed() {
        let input = b"WEBVTT\n\n00:00:04.000 --> 00:00:05.000\nCaption\n\n";
        assert_eq!(
            clip_vtt(input, 0, 10_000_000, false).unwrap(),
            b"WEBVTT\n\n"
        );
        assert!(clip_vtt(input, 10_000_000, 10_000_000, false).is_none());
        assert!(clip_vtt(input, 0, 610_000_000, false).is_none());
        assert!(
            clip_vtt(
                b"WEBVTT\n\n00:00:70.000 --> 00:01:00.000\nBad\n\n",
                0,
                20_000_000,
                false
            )
            .is_none()
        );
        assert!(
            clip_vtt(
                b"WEBVTT\n\nX-TIMESTAMP-MAP=broken\n\n",
                0,
                20_000_000,
                false
            )
            .is_none()
        );
        assert!(vtt_to_srt(b"WEBVTT\n\nbad\n").is_none());
    }
}
