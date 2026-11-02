//! Bounded, read-only preflight for raw Blu-ray PGS SUP tracks.
//! Packet and segment layout validation now includes bounded indexed-pixel
//! expansion for completed RLE objects. It does not interpret palettes,
//! resolve graphical composition, or render subtitles to a video frame.
use std::{collections::BTreeMap, io::Read};

pub(crate) const MAX_SUP_BYTES: u64 = 32 * 1024 * 1024;
const MAX_PGS_PACKETS: usize = 100_000;

pub(crate) fn header_valid(bytes: &[u8; 13], remaining: u64) -> bool {
    let length = u64::from(u16::from_be_bytes([bytes[11], bytes[12]]));
    &bytes[..2] == b"PG"
        && matches!(bytes[10], 0x14 | 0x15 | 0x16 | 0x17 | 0x80)
        && (bytes[10] != 0x80 || length == 0)
        && 13_u64
            .checked_add(length)
            .is_some_and(|end| end <= remaining)
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// A single PDS version may update several entries, but declaring the same
/// palette index twice in that packet leaves its rendered meaning ambiguous.
/// Reject ambiguous packets before reporting an image as validated.
fn palette_entries_unique(bytes: &[u8]) -> bool {
    let mut defined = [false; 256];
    for entry in bytes[2..].chunks_exact(5) {
        let index = usize::from(entry[0]);
        if defined[index] {
            return false;
        }
        defined[index] = true;
    }
    true
}

/// Validate the layout of each recognized segment without interpreting the
/// subtitle image. Narrow limits on complex compositions are deliberate:
/// unsupported data must not be advertised as known-compatible graphical
/// subtitles. Stream-copy itself never modifies or rewrites segment bytes.
fn segment_shape_valid(kind: u8, bytes: &[u8]) -> bool {
    match kind {
        // Palette ID + version, then zero or more five-byte color entries.
        0x14 => {
            bytes.len() >= 2
                && (bytes.len() - 2) % 5 == 0
                && (bytes.len() - 2) / 5 <= 256
                && palette_entries_unique(bytes)
        }
        // Object ID, version, sequence flags, and optionally image metadata.
        0x15 => {
            if bytes.len() < 4 || bytes[3] & 0x3f != 0 {
                return false;
            }
            if bytes[3] & 0x80 == 0 {
                return bytes.len() > 4;
            }
            if bytes.len() < 11 {
                return false;
            }
            let data_length = u32::from_be_bytes([0, bytes[4], bytes[5], bytes[6]]);
            data_length >= 4 && be16(&bytes[7..9]) > 0 && be16(&bytes[9..11]) > 0
        }
        // Canvas, frame rate, composition number/state, palette selection
        // and at most two object records (with optional eight-byte crop).
        0x16 => {
            if bytes.len() < 11
                || !(1..=8192).contains(&be16(&bytes[0..2]))
                || !(1..=8192).contains(&be16(&bytes[2..4]))
                || !matches!(bytes[7], 0x00 | 0x40 | 0x80)
                || !matches!(bytes[8], 0x00 | 0x80)
            {
                return false;
            }
            let count = usize::from(bytes[10]);
            if count > 2 {
                return false;
            }
            let mut offset = 11_usize;
            for _ in 0..count {
                if bytes.len().saturating_sub(offset) < 8 {
                    return false;
                }
                let flags = bytes[offset + 3];
                if flags & 0x3f != 0 {
                    return false;
                }
                let cropped = flags & 0x80 != 0;
                let record_len = if cropped { 16 } else { 8 };
                if bytes.len() - offset < record_len {
                    return false;
                }
                if cropped
                    && (be16(&bytes[offset + 12..offset + 14]) == 0
                        || be16(&bytes[offset + 14..offset + 16]) == 0)
                {
                    return false;
                }
                offset += record_len;
            }
            offset == bytes.len()
        }
        // Window count, then records of id,x,y,width,height (nine bytes).
        0x17 => {
            if bytes.is_empty() || !(1..=2).contains(&bytes[0]) {
                return false;
            }
            if bytes.len() != 1 + usize::from(bytes[0]) * 9 {
                return false;
            }
            let mut seen = [false; 256];
            for window in bytes[1..].chunks_exact(9) {
                let id = usize::from(window[0]);
                if seen[id] || be16(&window[5..7]) == 0 || be16(&window[7..9]) == 0 {
                    return false;
                }
                seen[id] = true;
            }
            true
        }
        // Display set terminator is strictly empty.
        0x80 => bytes.is_empty(),
        _ => false,
    }
}

/// Keep bitmap expansion bounded independently of the raw SUP file cap.
/// This is a decoder preflight, not a renderer or bitmap-to-PNG API.
const MAX_BITMAP_PIXELS: usize = 4 * 1024 * 1024;
const MAX_OBJECT_RLE_BYTES: usize = 4 * 1024 * 1024;

struct PendingObject {
    remaining: usize,
    width: u16,
    height: u16,
    compressed: Vec<u8>,
}

/// Expand the published PGS RLE word format into indexed pixels. A nonzero
/// octet is one literal pixel; zero introduces short/long transparent or
/// colored runs, or an explicit end-of-line marker. No palette interpretation
/// occurs here. Dimensions and output memory are checked before allocation.
fn decode_rle_bitmap(data: &[u8], width: u16, height: u16) -> Option<Vec<u8>> {
    let width = usize::from(width);
    let height = usize::from(height);
    let area = width.checked_mul(height)?;
    if width == 0 || height == 0 || area > MAX_BITMAP_PIXELS {
        return None;
    }
    let mut pixels = vec![0; area];
    let mut cursor = 0_usize;
    let mut written = 0_usize;
    let mut since_last_eol = false;
    while cursor < data.len() {
        if written == area {
            // A final EOL marker is allowed even if the last row was full.
            return (data.len() == cursor + 2 && data[cursor..] == [0, 0]).then_some(pixels);
        }
        let opcode = *data.get(cursor)?;
        cursor += 1;
        if opcode != 0 {
            pixels[written] = opcode;
            written += 1;
            since_last_eol = true;
            continue;
        }
        let control = *data.get(cursor)?;
        cursor += 1;
        if control == 0 {
            // A fully filled scanline followed by EOL must not skip
            // the next row; two consecutive EOLs encode an empty row.
            let remainder = written % width;
            if remainder != 0 {
                written += width - remainder;
            } else if !since_last_eol {
                written += width;
            }
            if written > area {
                return None;
            }
            since_last_eol = false;
            continue;
        }
        let mut count = usize::from(control & 0x3f);
        if control & 0x40 != 0 {
            count = count.checked_shl(8)? + usize::from(*data.get(cursor)?);
            cursor += 1;
        }
        if count == 0 {
            return None;
        }
        let color = if control & 0x80 != 0 {
            let value = *data.get(cursor)?;
            cursor += 1;
            value
        } else {
            0
        };
        let end = written.checked_add(count)?;
        // PGS run codes are scanline-local. A malformed RLE run must not
        // consume pixels from the next row merely because the flattened
        // output buffer still has sufficient capacity.
        if end > area || count > width - (written % width) {
            return None;
        }
        pixels[written..end].fill(color);
        written = end;
        since_last_eol = true;
    }
    (written == area).then_some(pixels)
}

/// Accumulate only a bounded number of compressed ODS fragments. On final
/// fragments, check the actual RLE decode as well as the declared byte count,
/// so corrupt image payloads cannot masquerade as compatible bitmaps.
fn object_fragment_valid(pending: &mut BTreeMap<u16, PendingObject>, bytes: &[u8]) -> bool {
    let id = be16(&bytes[..2]);
    let flags = bytes[3];
    let last = flags & 0x40 != 0;
    if flags & 0x80 != 0 {
        if pending.contains_key(&id) {
            return false;
        }
        let declared = u32::from_be_bytes([0, bytes[4], bytes[5], bytes[6]]);
        let Some(image_bytes) = declared.checked_sub(4).map(|size| size as usize) else {
            return false;
        };
        let width = be16(&bytes[7..9]);
        let height = be16(&bytes[9..11]);
        if image_bytes == 0
            || image_bytes > MAX_OBJECT_RLE_BYTES
            || usize::from(width) * usize::from(height) > MAX_BITMAP_PIXELS
            || bytes.len() - 11 > image_bytes
        {
            return false;
        }
        if last {
            return bytes.len() - 11 == image_bytes
                && decode_rle_bitmap(&bytes[11..], width, height).is_some();
        }
        if bytes.len() - 11 == image_bytes || pending.len() >= 64 {
            return false;
        }
        pending.insert(
            id,
            PendingObject {
                remaining: image_bytes - (bytes.len() - 11),
                width,
                height,
                compressed: bytes[11..].to_vec(),
            },
        );
        return true;
    }
    let Some(object) = pending.get_mut(&id) else {
        return false;
    };
    let fragment = &bytes[4..];
    if fragment.is_empty() || fragment.len() > object.remaining {
        return false;
    }
    object.remaining -= fragment.len();
    object.compressed.extend_from_slice(fragment);
    if last {
        if object.remaining != 0
            || decode_rle_bitmap(&object.compressed, object.width, object.height).is_none()
        {
            return false;
        }
        pending.remove(&id);
    } else if object.remaining == 0 {
        return false;
    }
    true
}

/// The total size, packet count, and per-packet allocation are bounded.
pub(crate) fn packets_valid<R: Read>(reader: &mut R, size: u64) -> bool {
    if !(13..=MAX_SUP_BYTES).contains(&size) {
        return false;
    }
    let mut offset = 0_u64;
    let mut packets = 0_usize;
    let mut ended_display_set = false;
    let mut payload = Vec::new();
    let mut pending_objects = BTreeMap::new();
    while offset < size {
        if packets >= MAX_PGS_PACKETS || size - offset < 13 {
            return false;
        }
        let mut header = [0_u8; 13];
        if reader.read_exact(&mut header).is_err() || !header_valid(&header, size - offset) {
            return false;
        }
        let length = usize::from(u16::from_be_bytes([header[11], header[12]]));
        payload.resize(length, 0);
        if reader.read_exact(&mut payload).is_err() || !segment_shape_valid(header[10], &payload) {
            return false;
        }
        if (header[10] == 0x15 && !object_fragment_valid(&mut pending_objects, &payload))
            || (header[10] == 0x80 && !pending_objects.is_empty())
        {
            return false;
        }
        let Some(next) = offset
            .checked_add(13)
            .and_then(|offset| offset.checked_add(length as u64))
        else {
            return false;
        };
        offset = next;
        ended_display_set = header[10] == 0x80;
        packets += 1;
    }
    packets > 0 && ended_display_set && pending_objects.is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(kind: u8, body: &[u8]) -> Vec<u8> {
        let mut bytes = b"PG".to_vec();
        bytes.extend_from_slice(&[0; 8]);
        bytes.push(kind);
        bytes.extend_from_slice(&(body.len() as u16).to_be_bytes());
        bytes.extend_from_slice(body);
        bytes
    }

    fn valid_clear_set() -> Vec<u8> {
        let mut bytes = packet(0x16, &[0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 0]);
        bytes.extend(packet(0x17, &[1, 0, 0, 0, 0, 0, 0, 32, 0, 32]));
        bytes.extend(packet(0x14, &[0, 0]));
        bytes.extend(packet(0x80, &[]));
        bytes
    }

    #[test]
    fn validates_structured_clear_display_sets_and_packet_boundaries() {
        let bytes = valid_clear_set();
        assert!(packets_valid(&mut bytes.as_slice(), bytes.len() as u64));
        let mut multiple = bytes.clone();
        multiple.extend_from_slice(&bytes);
        assert!(packets_valid(
            &mut multiple.as_slice(),
            multiple.len() as u64
        ));
        let mut unclosed = bytes.clone();
        unclosed.extend(packet(0x14, &[0, 0]));
        assert!(!packets_valid(
            &mut unclosed.as_slice(),
            unclosed.len() as u64
        ));
        let mut truncated = bytes.clone();
        truncated.pop();
        assert!(!packets_valid(
            &mut truncated.as_slice(),
            truncated.len() as u64
        ));
        let mut extra = bytes.clone();
        extra.extend_from_slice(b"bad");
        assert!(!packets_valid(&mut extra.as_slice(), extra.len() as u64));
        let mut bad_header = bytes.clone();
        bad_header[0] = b'X';
        assert!(!packets_valid(
            &mut bad_header.as_slice(),
            bad_header.len() as u64
        ));
    }

    #[test]
    fn fragmented_pgs_objects_must_complete_their_declared_rle_lengths() {
        let first = packet(0x15, &[0, 1, 0, 0x80, 0, 0, 6, 0, 2, 0, 1, 0xaa]);
        let last = packet(0x15, &[0, 1, 0, 0x40, 0xbb]);
        let mut complete = first.clone();
        complete.extend(&last);
        complete.extend(packet(0x80, &[]));
        assert!(packets_valid(
            &mut complete.as_slice(),
            complete.len() as u64
        ));

        let mut single = packet(0x15, &[0, 1, 0, 0xc0, 0, 0, 6, 0, 2, 0, 1, 0xaa, 0xbb]);
        assert!(!packets_valid(&mut single.as_slice(), single.len() as u64));
        single.extend(packet(0x80, &[]));
        assert!(packets_valid(&mut single.as_slice(), single.len() as u64));

        // A valid colored-run escape may span three ODS fragments.
        // Decode only after the final fragment, using the complete RLE data.
        let mut split_run = packet(0x15, &[0, 2, 0, 0x80, 0, 0, 7, 0, 3, 0, 1, 0]);
        split_run.extend(packet(0x15, &[0, 2, 0, 0, 0x83]));
        split_run.extend(packet(0x15, &[0, 2, 0, 0x40, 7]));
        split_run.extend(packet(0x80, &[]));
        assert!(packets_valid(
            &mut split_run.as_slice(),
            split_run.len() as u64
        ));
        // Correct fragment sizes cannot disguise a dangling RLE escape.
        let mut dangling_run = packet(0x15, &[0, 3, 0, 0x80, 0, 0, 6, 0, 3, 0, 1, 0]);
        dangling_run.extend(packet(0x15, &[0, 3, 0, 0x40, 0x83]));
        assert!(!packets_valid(
            &mut dangling_run.as_slice(),
            dangling_run.len() as u64
        ));

        // Incomplete, unmatched, repeated and oversized final fragments all
        // fail closed, including when a display set is otherwise well framed.
        let mut unfinished = first.clone();
        unfinished.extend(packet(0x80, &[]));
        let mut wrong_id = first.clone();
        wrong_id.extend(packet(0x15, &[0, 2, 0, 0x40, 0xbb]));
        let mut repeated = first.clone();
        repeated.extend(&first);
        let mut oversized = first.clone();
        oversized.extend(packet(0x15, &[0, 1, 0, 0x40, 0xbb, 0xcc]));
        let short_declared = packet(0x15, &[0, 1, 0, 0xc0, 0, 0, 7, 0, 2, 0, 1, 0xaa, 0xbb]);
        for bad in [
            first,
            unfinished,
            wrong_id,
            repeated,
            oversized,
            short_declared,
        ] {
            assert!(!packets_valid(&mut bad.as_slice(), bad.len() as u64));
        }
    }

    #[test]
    fn bitmap_rle_expansion_is_bounded_and_covers_literal_transparent_and_colored_runs() {
        assert_eq!(
            decode_rle_bitmap(&[0xaa, 0xbb], 2, 1),
            Some(vec![0xaa, 0xbb])
        );
        assert_eq!(decode_rle_bitmap(&[0, 0x83, 7], 3, 1), Some(vec![7; 3]));
        assert_eq!(
            decode_rle_bitmap(&[0, 0xc0, 65, 7], 65, 1),
            Some(vec![7; 65])
        );
        assert_eq!(decode_rle_bitmap(&[0, 0x03], 3, 1), Some(vec![0; 3]));
        assert_eq!(decode_rle_bitmap(&[0, 0x40, 65], 65, 1), Some(vec![0; 65]));
        assert_eq!(
            decode_rle_bitmap(&[0, 0, 0xaa, 0xbb], 2, 2),
            Some(vec![0, 0, 0xaa, 0xbb])
        );
        assert_eq!(
            decode_rle_bitmap(&[1, 2, 0, 0, 3, 4], 2, 2),
            Some(vec![1, 2, 3, 4])
        );
        assert_eq!(
            decode_rle_bitmap(&[0, 0, 0, 0, 3, 4], 2, 3),
            Some(vec![0, 0, 0, 0, 3, 4])
        );
        assert_eq!(
            decode_rle_bitmap(&[0xaa, 0xbb, 0, 0], 2, 1),
            Some(vec![0xaa, 0xbb])
        );
        for invalid in [
            &[0][..],
            &[0, 0x80, 0x2a],
            &[0, 0x83, 7],
            &[0, 0x40],
            &[0, 0xc0, 65],
            &[0xaa, 0xbb, 0xcc],
        ] {
            assert!(decode_rle_bitmap(invalid, 2, 1).is_none());
        }
        // A palette run crossing the end of a scanline is malformed even
        // if the flattened image has room for its declared pixel count.
        assert!(decode_rle_bitmap(&[0, 0x83, 7, 0xaa], 2, 2).is_none());
        assert!(decode_rle_bitmap(&[0xaa, 0, 0x02, 0xbb], 2, 2).is_none());
        assert!(decode_rle_bitmap(&[1], 4096, 4096).is_none());
        assert!(decode_rle_bitmap(&[1], 0, 1).is_none());
    }

    #[test]
    fn composition_flags_crops_and_window_ids_are_well_formed() {
        // These are segment-shape checks only. Valid crop placement against a
        // decoded object and palette composition require a later renderer.
        let mut composition = vec![
            0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 1, // PCS header
            0, 1, 0, 0x80, 0, 0, 0, 0, // cropped object
            0, 0, 0, 0, 0, 1, 0, 1, // source crop (1x1)
        ];
        assert!(segment_shape_valid(0x16, &composition));
        composition[14] = 0xc0; // forced and cropped
        assert!(segment_shape_valid(0x16, &composition));
        composition[14] = 0x01; // reserved flags
        assert!(!segment_shape_valid(0x16, &composition));
        composition[14] = 0x80;
        composition[23] = 0; // zero crop width
        composition[24] = 0;
        assert!(!segment_shape_valid(0x16, &composition));
        composition[24] = 1;
        composition[7] = 0x20; // unknown composition state
        assert!(!segment_shape_valid(0x16, &composition));
        composition[7] = 0x80;
        composition[8] = 0x40; // unknown palette-update flag
        assert!(!segment_shape_valid(0x16, &composition));

        let mut windows = vec![
            2, // two window records
            0, 0, 0, 0, 0, 0, 32, 0, 32, 1, 0, 0, 0, 0, 0, 32, 0, 32,
        ];
        assert!(segment_shape_valid(0x17, &windows));
        windows[10] = 0; // duplicate window ID
        assert!(!segment_shape_valid(0x17, &windows));
        windows[10] = 1;
        windows[16] = 0; // second window width is now zero
        assert!(!segment_shape_valid(0x17, &windows));

        // Exercise the same semantic boundary through the complete SUP
        // preflight used by authenticated raw subtitle HTTP delivery.
        windows[16] = 32;
        composition[8] = 0;
        let mut full = packet(0x16, &composition);
        let window_packet_offset = full.len();
        full.extend(packet(0x17, &windows));
        full.extend(packet(
            0x14,
            &[0, 0, 0xaa, 16, 128, 128, 255, 0xbb, 235, 128, 128, 255],
        ));
        full.extend(packet(
            0x15,
            &[0, 1, 0, 0xc0, 0, 0, 6, 0, 2, 0, 1, 0xaa, 0xbb],
        ));
        full.extend(packet(0x80, &[]));
        assert!(packets_valid(&mut full.as_slice(), full.len() as u64));

        let mut bad_pcs = full.clone();
        bad_pcs[13 + 14] = 1; // reserved composition-object flag
        assert!(!packets_valid(
            &mut bad_pcs.as_slice(),
            bad_pcs.len() as u64
        ));
        let mut bad_wds = full.clone();
        bad_wds[window_packet_offset + 13 + 10] = 0; // duplicate window ID
        assert!(!packets_valid(
            &mut bad_wds.as_slice(),
            bad_wds.len() as u64
        ));
    }

    #[test]
    fn rejects_malformed_palette_object_composition_and_window_layouts() {
        let cases: &[(u8, &[u8])] = &[
            (0x14, &[0]),
            (0x14, &[0, 0, 1]),
            (0x14, &[0, 0, 5, 76, 255, 85, 255, 5, 80, 255, 85, 255]),
            (0x15, &[0, 0, 0, 0x80]),
            (0x15, &[0, 0, 0, 0x80, 0, 0, 4, 0, 0, 0, 16]),
            (0x16, &[0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0]),
            (0x16, &[0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 3]),
            (0x16, &[0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 1]),
            (0x17, &[1, 0, 0]),
            (0x17, &[1, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
            (0x80, &[1]),
        ];
        for &(kind, payload) in cases {
            let bytes = packet(kind, payload);
            assert!(
                !packets_valid(&mut bytes.as_slice(), bytes.len() as u64),
                "segment kind: {kind:#04x}"
            );
        }
        assert!(segment_shape_valid(
            0x14,
            &[0, 0, 5, 76, 255, 85, 255, 6, 80, 255, 85, 255]
        ));
        let valid_first_object = [0, 1, 0, 0xc0, 0, 0, 8, 0, 8, 0, 8, 0xaa, 0xbb, 0xcc, 0xdd];
        assert!(segment_shape_valid(0x15, &valid_first_object));
        assert!(segment_shape_valid(
            0x16,
            &[
                0, 32, 0, 32, 0x10, 0, 0, 0x80, 0, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0
            ]
        ));
    }
}
