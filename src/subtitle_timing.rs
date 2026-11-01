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
    std::char::decode_utf16(units)
        .collect::<Result<String, _>>()
        .ok()
}

pub(crate) fn parse_stamp(stamp: &str) -> Option<u64> {
    let (clock, fraction) = stamp.split_once('.')?;
    if fraction.len() != 3 || !fraction.bytes().all(|digit| digit.is_ascii_digit()) {
        return None;
    }
    let fields = clock.split(':').collect::<Vec<_>>();
    let (hours, minutes, seconds) = match fields.as_slice() {
        [minutes, seconds] => ("00", *minutes, *seconds),
        [hours, minutes, seconds] => (*hours, *minutes, *seconds),
        _ => return None,
    };
    if !(2..=3).contains(&hours.len())
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

// Cue positioning is valid VTT syntax but has no SRT equivalent.
// Restrict the small supported grammar rather than reproducing malformed
// settings in a generated VTT segment. Each setting may occur only once.
fn cue_percent(value: &str) -> bool {
    let Some(raw) = value.strip_suffix('%') else {
        return false;
    };
    if raw.len() > 12 {
        return false;
    }
    let (whole, fraction) = raw.split_once('.').unwrap_or((raw, ""));
    if whole.is_empty()
        || !whole.bytes().all(|digit| digit.is_ascii_digit())
        || (raw.contains('.')
            && (fraction.is_empty() || !fraction.bytes().all(|digit| digit.is_ascii_digit())))
    {
        return false;
    }
    raw.parse::<f64>()
        .ok()
        .is_some_and(|percent| percent <= 100.0)
}

fn supported_cue_settings(settings: &str) -> bool {
    if settings.len() > 128 {
        return false;
    }
    let mut seen = [false; 5];
    for setting in settings.split_whitespace() {
        let Some((name, value)) = setting.split_once(':') else {
            return false;
        };
        let (slot, valid) = match name {
            "align" => (
                0,
                matches!(value, "start" | "center" | "end" | "left" | "right"),
            ),
            "vertical" => (1, matches!(value, "rl" | "lr")),
            "size" => (2, cue_percent(value)),
            "line" => {
                let (line, anchor) = value.split_once(',').unwrap_or((value, ""));
                let valid_anchor =
                    anchor.is_empty() || matches!(anchor, "start" | "center" | "end");
                let valid_line = cue_percent(line) || line.parse::<i32>().is_ok();
                (3, valid_anchor && valid_line)
            }
            "position" => {
                let (position, anchor) = value.split_once(',').unwrap_or((value, ""));
                let valid_anchor = anchor.is_empty()
                    || matches!(anchor, "line-left" | "center" | "line-right" | "auto");
                (4, cue_percent(position) && valid_anchor)
            }
            _ => return false,
        };
        if !valid || seen[slot] {
            return false;
        }
        seen[slot] = true;
    }
    true
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
    let prefix = normalized.strip_prefix("WEBVTT")?;
    let (header, body) = prefix.split_once("\n\n")?;
    if !header.is_empty()
        && (header.len() > 256
            || !(header.starts_with(' ') || header.starts_with('\t'))
            || header.chars().any(char::is_control))
    {
        return None;
    }
    let mut result = String::from("WEBVTT\n\n");
    let mut cues = 0_u32;
    for block in body.split("\n\n").filter(|part| !part.trim().is_empty()) {
        let mut lines = block.lines();
        let first = lines.next()?.trim();
        if first == "NOTE" || first.starts_with("NOTE ") {
            continue;
        }
        if matches!(first, "STYLE" | "REGION") {
            return None;
        }
        let timing = if first.contains(" --> ") {
            first
        } else if !first.is_empty() && first.len() <= 128 && !first.chars().any(char::is_control) {
            lines.next()?.trim()
        } else {
            return None;
        };
        let (start, end_with_settings) = timing.split_once(" --> ")?;
        let (end, settings) = end_with_settings
            .split_once(' ')
            .unwrap_or((end_with_settings, ""));
        if !supported_cue_settings(settings) {
            return None;
        }
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
            if !settings.is_empty() {
                result.push(' ');
                result.push_str(settings);
            }
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
        let (start, end_with_settings) = timing.split_once(" --> ")?;
        let (end, settings) = end_with_settings
            .split_once(' ')
            .unwrap_or((end_with_settings, ""));
        if parse_stamp(start).is_none()
            || parse_stamp(end).is_none()
            || !supported_cue_settings(settings)
        {
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

/// Convert a separately delivered WebVTT sidecar to the bounded SRT
/// subset. Accept ordinary cue identifiers, short timestamps and optional
/// single-line WEBVTT titles, but fail closed on advanced regions/styles,
/// malformed timing or unsupported settings. Neither rebase timestamps nor
/// depend on a VOD-window duration.
pub(crate) fn vtt_sidecar_to_srt(source: &[u8]) -> Option<Vec<u8>> {
    if source.len() > 4 * 1024 * 1024 {
        return None;
    }
    let source = std::str::from_utf8(source).ok()?.trim_start_matches('\u{feff}');
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let (header, body) = normalized.split_once("\n\n")?;
    if !(header == "WEBVTT" || header.starts_with("WEBVTT ") || header.starts_with("WEBVTT\t"))
        || header.len() > 256
        || header.contains('\n')
        || header.chars().any(|ch| ch.is_control() && ch != '\t')
    {
        return None;
    }
    let mut result = String::new();
    let mut cues = 0_u32;
    for block in body.split("\n\n").filter(|block| !block.trim().is_empty()) {
        let mut lines = block.lines();
        let first = lines.next()?.trim();
        if first == "NOTE" || first.starts_with("NOTE ") {
            continue;
        }
        if matches!(first, "STYLE" | "REGION") {
            return None;
        }
        let timing = if first.contains(" --> ") {
            first
        } else if !first.is_empty()
            && first.len() <= 128
            && !first.chars().any(char::is_control)
        {
            lines.next()?.trim()
        } else {
            return None;
        };
        let (start, end_with_settings) = timing.split_once(" --> ")?;
        let (end, settings) = end_with_settings
            .split_once(' ')
            .unwrap_or((end_with_settings, ""));
        let start = parse_stamp(start)?;
        let end = parse_stamp(end)?;
        if end <= start || !supported_cue_settings(settings) {
            return None;
        }
        let caption = lines.collect::<Vec<_>>().join("\n");
        if caption.trim().is_empty()
            || caption.chars().any(|ch| ch.is_control() && ch != '\n' && ch != '\t')
        {
            return None;
        }
        cues += 1;
        if cues > 100_000 {
            return None;
        }
        result.push_str(&format!(
            "{cues}\n{} --> {}\n{}\n\n",
            write_stamp(start).replace('.', ","),
            write_stamp(end).replace('.', ","),
            caption.trim_end()
        ));
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
    fn webvtt_sidecar_converts_short_cues_ids_and_positions_without_rebasing() {
        let input = b"\xef\xbb\xbfWEBVTT - Local captions\r\n\r\nNOTE from the editor\r\nignored\r\n\r\nintro\r\n00:01.000 --> 00:03.500 align:start position:50.5%\r\nHola!\r\n\r\n00:00:04.000 --> 00:00:05.000\r\nSecond\r\n";
        assert_eq!(
            vtt_sidecar_to_srt(input).unwrap(),
            b"1\n00:00:01,000 --> 00:00:03,500\nHola!\n\n2\n00:00:04,000 --> 00:00:05,000\nSecond\n\n"
        );
        assert_eq!(vtt_sidecar_to_srt(b"WEBVTT\n\n").unwrap(), b"");
    }

    #[test]
    fn webvtt_sidecar_rejects_advanced_and_malformed_cues() {
        for input in [
            "WEBVTT\nX-TIMESTAMP-MAP=MPEGTS:0,LOCAL:00:00:00.000\n\n",
            "WEBVTT\n\nSTYLE\n::cue { color: red }\n\n",
            "WEBVTT\n\nREGION\nid:r1\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:01.000\nZero\n\n",
            "WEBVTT\n\n00:00:03.000 --> 00:00:01.000\nReversed\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 align:invalid\nBad\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000 align:start align:end\nBad\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\n\n",
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nInvalid\x00cue\n\n",
            "not WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nBad\n\n",
        ] {
            assert!(vtt_sidecar_to_srt(input.as_bytes()).is_none(), "{input:?}");
        }
    }

    #[test]
    fn srt_decoding_accepts_bom_utf16_without_replacing_invalid_data() {
        let caption = "1\n00:00:00,000 --> 00:00:01,000\nOlá\n";
        let mut utf16le = vec![0xff, 0xfe];
        utf16le.extend(caption.encode_utf16().flat_map(u16::to_le_bytes));
        let mut utf16be = vec![0xfe, 0xff];
        utf16be.extend(caption.encode_utf16().flat_map(u16::to_be_bytes));
        assert_eq!(decode_text_subtitle(&utf16le).as_deref(), Some(caption));
        assert_eq!(decode_text_subtitle(&utf16be).as_deref(), Some(caption));
        assert_eq!(
            decode_text_subtitle(caption.as_bytes()).as_deref(),
            Some(caption)
        );
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
    fn ordinary_vtt_cue_identifiers_short_timestamps_and_settings_survive() {
        let vtt = b"WEBVTT - Local captions\n\nNOTE From the editor\nignored comment\n\nintro\n00:01.000 --> 00:03.000 align:start\nHello!\n\n";
        let result = clip_vtt(vtt, 15_000_000, 25_000_000, false).unwrap();
        assert_eq!(
            result,
            b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000 align:start\nHello!\n\n"
        );
        // SRT has no cue-positioning attributes: discard only the settings,
        // preserving the clipped timestamps and caption text.
        assert_eq!(
            vtt_to_srt(&result).unwrap(),
            b"1\n00:00:00,000 --> 00:00:01,000\nHello!\n\n"
        );
        assert!(
            vtt_to_srt(b"WEBVTT\n\n00:00:00.000 --> 00:00:01.000 unknown:setting\nHello!\n\n")
                .is_none()
        );
        assert!(
            clip_vtt(
                b"WEBVTT\n\nSTYLE\n::cue{color:red}\n\n",
                0,
                20_000_000,
                false
            )
            .is_none()
        );
        assert!(
            clip_vtt(
                b"WEBVTT\n\nfoo\n00:01.000 --> 00:03.000 invalid:bar\nHi\n\n",
                0,
                20_000_000,
                false
            )
            .is_none()
        );
    }

    #[test]
    fn cue_settings_accept_valid_positions_and_reject_malformed_values() {
        let valid = b"WEBVTT\n\n00:00:01.000 --> 00:00:03.000 align:start line:-2 position:50.5%,line-left size:80.25% vertical:rl\nCaption\n\n";
        let window = clip_vtt(valid, 10_000_000, 30_000_000, false).unwrap();
        assert!(
            std::str::from_utf8(&window)
                .unwrap()
                .contains("position:50.5%,line-left")
        );
        assert_eq!(
            vtt_to_srt(&window).unwrap(),
            b"1\n00:00:00,000 --> 00:00:02,000\nCaption\n\n"
        );
        for invalid in [
            "align:",
            "align:wat",
            "align:start align:end",
            "line:nan",
            "size:101%",
            "size:100.01%",
            "size:1e2%",
            "size:20.%",
            "size:.5%",
            "size:20.0.1%",
            "size:-1%",
            "position:30%,unknown",
            "vertical:down",
            "line:2,invalid",
            "size:90%,size:50%",
            "no-colon",
        ] {
            let source = format!("WEBVTT\n\n00:00:01.000 --> 00:00:03.000 {invalid}\nCaption\n\n");
            assert!(
                clip_vtt(source.as_bytes(), 0, 30_000_000, false).is_none(),
                "{invalid}"
            );
        }
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
