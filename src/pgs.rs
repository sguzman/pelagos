//! Bounded, read-only structural preflight for raw Blu-ray PGS SUP tracks.
//! This validates packet framing and the supported segment *shapes* before
//! delivery, not palettes, RLE pixels, composition references, or rendering.
use std::io::Read;

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

/// Validate the layout of each recognized segment without interpreting the
/// subtitle image. Narrow limits on complex compositions are deliberate:
/// unsupported data must not be advertised as known-compatible graphical
/// subtitles. Stream-copy itself never modifies or rewrites segment bytes.
fn segment_shape_valid(kind: u8, bytes: &[u8]) -> bool {
    match kind {
        // Palette ID + version, then zero or more five-byte color entries.
        0x14 => bytes.len() >= 2 && (bytes.len() - 2) % 5 == 0 && (bytes.len() - 2) / 5 <= 256,
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
                let cropped = bytes[offset + 3] & 0x80 != 0;
                offset += if cropped { 16 } else { 8 };
                if offset > bytes.len() {
                    return false;
                }
            }
            offset == bytes.len()
        }
        // Window count, then records of id,x,y,width,height (nine bytes).
        0x17 => {
            if bytes.is_empty() || !(1..=2).contains(&bytes[0]) {
                return false;
            }
            bytes.len() == 1 + usize::from(bytes[0]) * 9
                && bytes[1..]
                    .chunks_exact(9)
                    .all(|window| be16(&window[5..7]) > 0 && be16(&window[7..9]) > 0)
        }
        // Display set terminator is strictly empty.
        0x80 => bytes.is_empty(),
        _ => false,
    }
}

/// The total size, packet count, and per-packet allocation are bounded.
pub(crate) fn packets_valid<R: Read>(reader: &mut R, size: u64) -> bool {
    if !(13..=MAX_SUP_BYTES).contains(&size) {
        return false;
    }
    let mut offset = 0_u64;
    let mut packets = 0_usize;
    let mut payload = Vec::new();
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
        let Some(next) = offset
            .checked_add(13)
            .and_then(|offset| offset.checked_add(length as u64))
        else {
            return false;
        };
        offset = next;
        packets += 1;
    }
    packets > 0
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
        bad_header[2] = b'X';
        assert!(!packets_valid(
            &mut bad_header.as_slice(),
            bad_header.len() as u64
        ));
    }

    #[test]
    fn rejects_malformed_palette_object_composition_and_window_layouts() {
        let cases: &[(u8, &[u8])] = &[
            (0x14, &[0]),
            (0x14, &[0, 0, 1]),
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
