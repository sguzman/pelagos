//! Experimental bounded rasterization of one complete PGS display set.
//!
//! This is an isolated preparation step for subtitle composition. It does
//! not schedule PGS presentation timestamps, retain palettes across epochs,
//! burn graphics into HLS, or claim unchanged Jellyfin-client compatibility.
//! Raw SUP transport remains owned by `pgs` and is not rewritten here.
use crate::pgs::{MAX_BITMAP_PIXELS, decode_rle_bitmap, packets_valid};
use std::collections::BTreeMap;

/// A straight-alpha RGBA8888 subtitle graphics plane, not a video frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RgbaDisplaySet {
    pub width: u16,
    pub height: u16,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy)]
struct ObjectRef {
    id: u16,
    window: u8,
    x: usize,
    y: usize,
    crop: Option<(usize, usize, usize, usize)>,
}

struct Presentation {
    width: u16,
    height: u16,
    palette_id: u8,
    objects: Vec<ObjectRef>,
}

struct ObjectPixels {
    width: usize,
    height: usize,
    indices: Vec<u8>,
}

struct PendingPixels {
    width: u16,
    height: u16,
    compressed: Vec<u8>,
}

#[derive(Clone, Copy)]
struct Window {
    x: usize,
    y: usize,
    width: usize,
    height: usize,
}

/// Decoder state kept within one PGS epoch. Object pixels are separately
/// bounded in addition to the compressed SUP input and returned RGBA planes.
#[derive(Default)]
struct EpochState {
    geometry: Option<(u16, u16)>,
    windows: BTreeMap<u8, Window>,
    palettes: BTreeMap<u8, [[u8; 4]; 256]>,
    objects: BTreeMap<u16, ObjectPixels>,
}

impl EpochState {
    fn reset(&mut self) {
        self.geometry = None;
        self.windows.clear();
        self.palettes.clear();
        self.objects.clear();
    }

    fn insert_object(&mut self, id: u16, object: ObjectPixels) -> Option<()> {
        const MAX_EPOCH_OBJECT_PIXELS: usize = 8 * 1024 * 1024;
        let current = self.objects.get(&id).map_or(0, |item| item.indices.len());
        let used = self
            .objects
            .values()
            .try_fold(0_usize, |sum, item| sum.checked_add(item.indices.len()))?;
        let next = used
            .checked_sub(current)?
            .checked_add(object.indices.len())?;
        if next > MAX_EPOCH_OBJECT_PIXELS || (current == 0 && self.objects.len() >= 64) {
            return None;
        }
        self.objects.insert(id, object);
        Some(())
    }
}

fn be16(bytes: &[u8]) -> u16 {
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// Limited-range YCrCb palette values to RGBA, using BT.601 for SD
/// compositions (up to 576 lines) and BT.709 above that height.
/// Undefined palette indices stay fully transparent.
fn palette_rgba(y: u8, cr: u8, cb: u8, alpha: u8, height: u16) -> [u8; 4] {
    let luma = (f64::from(y) - 16.0) * (255.0 / 219.0);
    let cr = f64::from(cr) - 128.0;
    let cb = f64::from(cb) - 128.0;
    let (r, g, b) = if height <= 576 {
        (
            luma + 1.402 * cr,
            luma - 0.344_136 * cb - 0.714_136 * cr,
            luma + 1.772 * cb,
        )
    } else {
        (
            luma + 1.574_8 * cr,
            luma - 0.187_324 * cb - 0.468_124 * cr,
            luma + 1.855_6 * cb,
        )
    };
    [
        r.round().clamp(0.0, 255.0) as u8,
        g.round().clamp(0.0, 255.0) as u8,
        b.round().clamp(0.0, 255.0) as u8,
        alpha,
    ]
}

/// Overlay straight-alpha source pixels, keeping the output straight-alpha
/// for eventual blending onto a video frame.
fn over(dst: &mut [u8], src: [u8; 4]) {
    let src_alpha = u32::from(src[3]);
    if src_alpha == 0 {
        return;
    }
    let dst_alpha = u32::from(dst[3]);
    let inverse = 255 - src_alpha;
    let output_alpha = src_alpha + (dst_alpha * inverse + 127) / 255;
    if output_alpha == 0 {
        return;
    }
    let denominator = output_alpha * 255;
    for channel in 0..3 {
        let numerator = u32::from(src[channel]) * src_alpha * 255
            + u32::from(dst[channel]) * dst_alpha * inverse;
        dst[channel] = ((numerator + denominator / 2) / denominator).min(255) as u8;
    }
    dst[3] = output_alpha as u8;
}

/// Decode and compose one validated, self-contained display set into a
/// transparent plane. It deliberately refuses display sets whose referenced
/// objects or palette require previous sets. That is an unsupported caller
/// mode, not a reason to reject their ordinary raw SUP delivery.
pub fn render_single_display_set(bytes: &[u8]) -> Option<RgbaDisplaySet> {
    render_with_state(bytes, &mut EpochState::default())
}

fn render_with_state(bytes: &[u8], state: &mut EpochState) -> Option<RgbaDisplaySet> {
    let mut validation = bytes;
    if !packets_valid(&mut validation, bytes.len() as u64) {
        return None;
    }
    let mut position = 0;
    let mut presentation: Option<Presentation> = None;
    let mut pending = BTreeMap::<u16, PendingPixels>::new();
    let mut ended = false;

    while position < bytes.len() {
        let header = &bytes[position..position + 13];
        let length = usize::from(be16(&header[11..13]));
        let segment = &bytes[position + 13..position + 13 + length];
        position += 13 + length;
        if ended {
            // One display set per call, not an implicit timeline decoder.
            return None;
        }
        match header[10] {
            0x16 => {
                if presentation.is_some() {
                    return None;
                }
                if segment[7] & 0xc0 != 0 {
                    state.reset();
                }
                let width = be16(&segment[0..2]);
                let height = be16(&segment[2..4]);
                if usize::from(width).checked_mul(usize::from(height))? > MAX_BITMAP_PIXELS
                    || state.geometry.is_some_and(|previous| previous != (width, height))
                {
                    return None;
                }
                state.geometry = Some((width, height));
                let mut refs = Vec::with_capacity(usize::from(segment[10]));
                let mut offset = 11;
                for _ in 0..segment[10] {
                    let crop = if segment[offset + 3] & 0x80 != 0 {
                        Some((
                            usize::from(be16(&segment[offset + 8..offset + 10])),
                            usize::from(be16(&segment[offset + 10..offset + 12])),
                            usize::from(be16(&segment[offset + 12..offset + 14])),
                            usize::from(be16(&segment[offset + 14..offset + 16])),
                        ))
                    } else {
                        None
                    };
                    refs.push(ObjectRef {
                        id: be16(&segment[offset..offset + 2]),
                        window: segment[offset + 2],
                        x: usize::from(be16(&segment[offset + 4..offset + 6])),
                        y: usize::from(be16(&segment[offset + 6..offset + 8])),
                        crop,
                    });
                    offset += if crop.is_some() { 16 } else { 8 };
                }
                presentation = Some(Presentation {
                    width,
                    height,
                    palette_id: segment[9],
                    objects: refs,
                });
            }
            0x17 => {
                for record in segment[1..].chunks_exact(9) {
                    state.windows.insert(
                        record[0],
                        Window {
                            x: usize::from(be16(&record[1..3])),
                            y: usize::from(be16(&record[3..5])),
                            width: usize::from(be16(&record[5..7])),
                            height: usize::from(be16(&record[7..9])),
                        },
                    );
                }
            }
            0x14 => {
                let pcs = presentation.as_ref()?;
                let id = segment[0];
                if !state.palettes.contains_key(&id) && state.palettes.len() >= 8 {
                    return None;
                }
                let colors = state.palettes.entry(id).or_insert([[0_u8; 4]; 256]);
                for entry in segment[2..].chunks_exact(5) {
                    colors[usize::from(entry[0])] =
                        palette_rgba(entry[1], entry[2], entry[3], entry[4], pcs.height);
                }
            }
            0x15 => {
                let id = be16(&segment[0..2]);
                let flags = segment[3];
                if flags & 0x80 != 0 {
                    let width = be16(&segment[7..9]);
                    let height = be16(&segment[9..11]);
                    if flags & 0x40 != 0 {
                        state.insert_object(
                            id,
                            ObjectPixels {
                                width: usize::from(width),
                                height: usize::from(height),
                                indices: decode_rle_bitmap(&segment[11..], width, height)?,
                            },
                        )?;
                    } else {
                        pending.insert(
                            id,
                            PendingPixels {
                                width,
                                height,
                                compressed: segment[11..].to_vec(),
                            },
                        );
                    }
                } else {
                    let entry = pending.get_mut(&id)?;
                    entry.compressed.extend_from_slice(&segment[4..]);
                    if flags & 0x40 != 0 {
                        let complete = pending.remove(&id)?;
                        state.insert_object(
                            id,
                            ObjectPixels {
                                width: usize::from(complete.width),
                                height: usize::from(complete.height),
                                indices: decode_rle_bitmap(
                                    &complete.compressed,
                                    complete.width,
                                    complete.height,
                                )?,
                            },
                        )?;
                    }
                }
            }
            0x80 => ended = true,
            _ => return None,
        }
    }

    let pcs = presentation?;
    let width = usize::from(pcs.width);
    let height = usize::from(pcs.height);
    let mut rgba = vec![0_u8; width.checked_mul(height)?.checked_mul(4)?];
    let colors = if pcs.objects.is_empty() {
        None
    } else {
        Some(state.palettes.get(&pcs.palette_id)?)
    };
    for reference in pcs.objects {
        let object = state.objects.get(&reference.id)?;
        let window = state.windows.get(&reference.window)?;
        let (crop_x, crop_y, crop_w, crop_h) =
            reference
                .crop
                .unwrap_or((0, 0, object.width, object.height));
        if crop_x.checked_add(crop_w)? > object.width || crop_y.checked_add(crop_h)? > object.height
        {
            return None;
        }
        let window_right = window.x.saturating_add(window.width).min(width);
        let window_bottom = window.y.saturating_add(window.height).min(height);
        for row in 0..crop_h {
            let dest_y = reference.y + row;
            if dest_y >= height || dest_y < window.y || dest_y >= window_bottom {
                continue;
            }
            for col in 0..crop_w {
                let dest_x = reference.x + col;
                if dest_x >= width || dest_x < window.x || dest_x >= window_right {
                    continue;
                }
                let index =
                    usize::from(object.indices[(crop_y + row) * object.width + crop_x + col]);
                let offset = (dest_y * width + dest_x) * 4;
                over(&mut rgba[offset..offset + 4], colors?[index]);
            }
        }
    }
    Some(RgbaDisplaySet {
        width: pcs.width,
        height: pcs.height,
        rgba,
    })
}

/// A separately rasterized self-contained display set and its presentation
/// time. The final duration remains unknown until a subsequent clear/update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedRgbaDisplaySet {
    pub pts_90khz: u32,
    pub duration_90khz: Option<u32>,
    pub image: RgbaDisplaySet,
}

const MAX_TIMELINE_FRAMES: usize = 16;
const MAX_TIMELINE_RGBA_BYTES: usize = 64 * 1024 * 1024;

/// Decode a small sequence of independent display sets, retaining only
/// the stream's actual PCS timestamps and bounded RGBA snapshots. This
/// experimental subset requires every displayed image to carry its own
/// complete palette and object data. It does not support epoch reuse,
/// 32-bit PTS rollover, live streaming, or inferred final cue durations.
pub fn render_self_contained_timeline(bytes: &[u8]) -> Option<Vec<TimedRgbaDisplaySet>> {
    render_timeline(bytes, false)
}

/// Decode a bounded sequence of PGS display sets retaining palette, window,
/// and object definitions between normal compositions within an epoch.
/// Epoch starts/acquisition points reset the cache; the same strict 16-frame,
/// 64-MiB output budget applies. No video burn-in or client parity is implied.
pub fn render_epoch_timeline(bytes: &[u8]) -> Option<Vec<TimedRgbaDisplaySet>> {
    render_timeline(bytes, true)
}

fn render_timeline(bytes: &[u8], keep_epoch_state: bool) -> Option<Vec<TimedRgbaDisplaySet>> {
    let mut validation = bytes;
    if !packets_valid(&mut validation, bytes.len() as u64) {
        return None;
    }
    let mut frames: Vec<TimedRgbaDisplaySet> = Vec::new();
    let mut epoch = EpochState::default();
    let mut total_rgba_bytes = 0_usize;
    let mut position = 0_usize;
    let mut display_start = 0_usize;
    let mut pts = None;
    while position < bytes.len() {
        let kind = bytes[position + 10];
        let length = usize::from(be16(&bytes[position + 11..position + 13]));
        if kind == 0x16 {
            if pts.is_some() {
                return None;
            }
            pts = Some(u32::from_be_bytes([
                bytes[position + 2],
                bytes[position + 3],
                bytes[position + 4],
                bytes[position + 5],
            ]));
        }
        position += 13 + length;
        if kind == 0x80 {
            if frames.len() >= MAX_TIMELINE_FRAMES {
                return None;
            }
            let stamp = pts.take()?;
            let image = if keep_epoch_state {
                render_with_state(&bytes[display_start..position], &mut epoch)?
            } else {
                render_single_display_set(&bytes[display_start..position])?
            };
            total_rgba_bytes = total_rgba_bytes.checked_add(image.rgba.len())?;
            if total_rgba_bytes > MAX_TIMELINE_RGBA_BYTES {
                return None;
            }
            if let Some(previous) = frames.last_mut() {
                previous.duration_90khz = Some(stamp.checked_sub(previous.pts_90khz)?);
                if previous.duration_90khz == Some(0) {
                    return None;
                }
            }
            frames.push(TimedRgbaDisplaySet {
                pts_90khz: stamp,
                duration_90khz: None,
                image,
            });
            display_start = position;
        }
    }
    (display_start == bytes.len() && !frames.is_empty()).then_some(frames)
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

    fn sample(cropped: bool) -> Vec<u8> {
        let mut pcs = vec![0, 4, 0, 2, 0x10, 0, 0, 0x80, 0, 0, 1];
        pcs.extend_from_slice(&[
            0,
            1,
            0,
            if cropped { 0x80 } else { 0 },
            0,
            if cropped { 3 } else { 1 },
            0,
            if cropped { 0 } else { 1 },
        ]);
        if cropped {
            pcs.extend_from_slice(&[0, 1, 0, 0, 0, 1, 0, 1]);
        }
        let mut bytes = packet(0x16, &pcs);
        bytes.extend(packet(0x17, &[1, 0, 0, 0, 0, 0, 0, 4, 0, 2]));
        bytes.extend(packet(
            0x14,
            &[0, 0, 0, 235, 128, 128, 255, 1, 16, 128, 128, 255],
        ));
        // First pixel uses palette index zero; second is literal index one.
        bytes.extend(packet(
            0x15,
            &[0, 1, 0, 0xc0, 0, 0, 8, 0, 2, 0, 1, 0, 0x81, 0, 1],
        ));
        bytes.extend(packet(0x80, &[]));
        bytes
    }

    #[test]
    fn palette_conversion_and_alpha_are_bounded() {
        assert_eq!(palette_rgba(16, 128, 128, 255, 480), [0, 0, 0, 255]);
        assert_eq!(palette_rgba(235, 128, 128, 128, 1080), [255, 255, 255, 128]);
        assert_eq!(palette_rgba(235, 128, 128, 0, 1080), [255, 255, 255, 0]);
        assert_eq!(palette_rgba(16, 128, 255, 255, 1080)[2], 236);
        assert_eq!(palette_rgba(16, 255, 128, 255, 1080)[0], 200);
    }

    #[test]
    fn places_rle_pixels_into_rgba_plane_and_crops_at_source() {
        let full = render_single_display_set(&sample(false)).unwrap();
        assert_eq!((full.width, full.height), (4, 2));
        assert_eq!(full.rgba.len(), 32);
        assert_eq!(&full.rgba[0..4], &[0, 0, 0, 0]);
        assert_eq!(&full.rgba[20..24], &[255, 255, 255, 255]);
        assert_eq!(&full.rgba[24..28], &[0, 0, 0, 255]);

        let cropped = render_single_display_set(&sample(true)).unwrap();
        assert_eq!(&cropped.rgba[12..16], &[0, 0, 0, 255]);
        assert!(cropped.rgba[..12].iter().all(|value| *value == 0));
    }

    #[test]
    fn rejects_incomplete_references_and_multiple_display_sets() {
        let valid = sample(false);
        let mut missing_palette = valid.clone();
        // Named palette ID 7 is absent, even though a different PDS exists.
        missing_palette[13 + 9] = 7;
        assert!(render_single_display_set(&missing_palette).is_none());

        let mut missing_window = valid.clone();
        missing_window[13 + 11 + 2] = 1;
        assert!(render_single_display_set(&missing_window).is_none());

        let mut missing_object = valid.clone();
        missing_object[13 + 11 + 1] = 2;
        assert!(render_single_display_set(&missing_object).is_none());

        let mut two = valid.clone();
        two.extend(valid);
        assert!(render_single_display_set(&two).is_none());
    }

    #[test]
    fn fragmented_objects_and_incremental_palette_updates_render_identically() {
        let original = sample(false);
        let ods_start = original.len() - (13 + 15) - 13;
        let mut fragmented = original[..ods_start].to_vec();
        fragmented.extend(packet(0x15, &[0, 1, 0, 0x80, 0, 0, 8, 0, 2, 0, 1, 0]));
        fragmented.extend(packet(0x15, &[0, 1, 0, 0x40, 0x81, 0, 1]));
        fragmented.extend(packet(0x80, &[]));
        assert_eq!(
            render_single_display_set(&fragmented),
            render_single_display_set(&original)
        );

        // A second PDS for the selected palette updates only one entry.
        // The other color remains white and the updated color turns clear.
        let mut changed = original.clone();
        let end_offset = changed.len() - 13;
        changed.splice(
            end_offset..end_offset,
            packet(0x14, &[0, 1, 1, 16, 128, 128, 0]),
        );
        let frame = render_single_display_set(&changed).unwrap();
        assert_eq!(&frame.rgba[20..24], &[255, 255, 255, 255]);
        assert_eq!(&frame.rgba[24..28], &[0, 0, 0, 0]);
    }

    #[test]
    fn window_clips_graphics_without_relocating_the_presentation() {
        let mut bytes = sample(false);
        let pcs_len = usize::from(be16(&bytes[11..13]));
        let wds_body = 13 + pcs_len + 13;
        bytes[wds_body + 3] = 2; // window begins at x=2
        bytes[wds_body + 7] = 1; // one-pixel window width
        let frame = render_single_display_set(&bytes).unwrap();
        assert_eq!(&frame.rgba[20..24], &[0, 0, 0, 0]);
        assert_eq!(&frame.rgba[24..28], &[0, 0, 0, 255]);
    }

    #[test]
    fn layered_objects_apply_straight_alpha_in_composition_order() {
        let mut pcs = vec![0, 2, 0, 1, 0x10, 0, 0, 0x80, 0, 0, 2];
        pcs.extend_from_slice(&[0, 1, 0, 0, 0, 0, 0, 0]);
        pcs.extend_from_slice(&[0, 2, 0, 0, 0, 0, 0, 0]);
        let mut bytes = packet(0x16, &pcs);
        bytes.extend(packet(0x17, &[1, 0, 0, 0, 0, 0, 0, 2, 0, 1]));
        bytes.extend(packet(
            0x14,
            &[0, 0, 1, 235, 128, 128, 255, 2, 16, 128, 128, 128],
        ));
        bytes.extend(packet(0x15, &[0, 1, 0, 0xc0, 0, 0, 6, 0, 2, 0, 1, 1, 1]));
        bytes.extend(packet(0x15, &[0, 2, 0, 0xc0, 0, 0, 6, 0, 2, 0, 1, 2, 2]));
        bytes.extend(packet(0x80, &[]));
        let frame = render_single_display_set(&bytes).unwrap();
        assert_eq!((frame.width, frame.height), (2, 1));
        assert_eq!(frame.rgba, vec![127, 127, 127, 255, 127, 127, 127, 255]);
    }

    #[test]
    fn clear_composition_renders_transparent_and_invalid_crop_fails_closed() {
        let mut clear = packet(0x16, &[0, 2, 0, 1, 0x10, 0, 0, 0x80, 0, 0, 0]);
        clear.extend(packet(0x80, &[]));
        assert_eq!(render_single_display_set(&clear).unwrap().rgba, vec![0; 8]);
        let mut bad_crop = sample(true);
        bad_crop[13 + 11 + 8 + 1] = 2; // source crop x now beyond bitmap
        assert!(render_single_display_set(&bad_crop).is_none());
    }

    #[test]
    fn timestamps_derive_cue_duration_from_next_clear_composition() {
        let mut first = sample(false);
        first[2..6].copy_from_slice(&90_000_u32.to_be_bytes());
        let mut clear = packet(0x16, &[0, 4, 0, 2, 0x10, 0, 1, 0, 0, 0, 0]);
        clear[2..6].copy_from_slice(&180_000_u32.to_be_bytes());
        clear.extend(packet(0x80, &[]));
        let mut stream = first.clone();
        stream.extend(&clear);
        let frames = render_self_contained_timeline(&stream).unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].pts_90khz, 90_000);
        assert_eq!(frames[0].duration_90khz, Some(90_000));
        assert_eq!(&frames[0].image.rgba[20..24], &[255, 255, 255, 255]);
        assert_eq!(frames[1].pts_90khz, 180_000);
        assert_eq!(frames[1].duration_90khz, None);
        assert!(frames[1].image.rgba.iter().all(|value| *value == 0));

        clear[2..6].copy_from_slice(&90_000_u32.to_be_bytes());
        let mut repeated_pts = first.clone();
        repeated_pts.extend(&clear);
        assert!(render_self_contained_timeline(&repeated_pts).is_none());

        clear[2..6].copy_from_slice(&1_u32.to_be_bytes());
        let mut reversed_pts = first;
        reversed_pts.extend(clear);
        assert!(render_self_contained_timeline(&reversed_pts).is_none());
    }

    #[test]
    fn epoch_cache_reuses_palettes_windows_and_objects_until_reset() {
        let mut first = sample(false);
        first[2..6].copy_from_slice(&90_000_u32.to_be_bytes());
        let mut reused = packet(
            0x16,
            &[0, 4, 0, 2, 0x10, 0, 1, 0, 0, 0, 1, 0, 1, 0, 0, 0, 1, 0, 1],
        );
        reused[2..6].copy_from_slice(&135_000_u32.to_be_bytes());
        reused.extend(packet(0x80, &[]));
        let mut clear = packet(0x16, &[0, 4, 0, 2, 0x10, 0, 2, 0, 0, 0, 0]);
        clear[2..6].copy_from_slice(&180_000_u32.to_be_bytes());
        clear.extend(packet(0x80, &[]));

        let mut stream = first.clone();
        stream.extend(&reused);
        stream.extend(&clear);
        assert!(render_self_contained_timeline(&stream).is_none());
        let frames = render_epoch_timeline(&stream).unwrap();
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].image, frames[1].image);
        assert_eq!(frames[0].duration_90khz, Some(45_000));
        assert_eq!(frames[1].duration_90khz, Some(45_000));
        assert!(frames[2].image.rgba.iter().all(|component| *component == 0));

        // An epoch-start PCS must not reuse the previous epoch's definitions.
        let mut reset = reused.clone();
        reset[13 + 7] = 0x80;
        let mut missing_after_reset = first.clone();
        missing_after_reset.extend(reset);
        assert!(render_epoch_timeline(&missing_after_reset).is_none());

        // A changed canvas without an epoch reset cannot safely reuse
        // palette colors selected under another video color matrix.
        let mut drifting = reused.clone();
        drifting[13 + 1] = 5;
        let mut changed_geometry = first.clone();
        changed_geometry.extend(drifting);
        assert!(render_epoch_timeline(&changed_geometry).is_none());

        // An independent epoch-start composition can establish new geometry.
        let mut new_epoch = sample(false);
        new_epoch[13 + 1] = 5;
        new_epoch[2..6].copy_from_slice(&180_000_u32.to_be_bytes());
        let mut restarted = first.clone();
        restarted.extend(new_epoch);
        assert_eq!(render_epoch_timeline(&restarted).unwrap()[1].image.width, 5);

        // An incremental PDS may change one cached palette index, while
        // the previous object pixels and window remain untouched.
        let mut changed = reused[..reused.len() - 13].to_vec();
        changed.extend(packet(0x14, &[0, 1, 1, 235, 128, 128, 255]));
        changed.extend(packet(0x80, &[]));
        let mut updated = first;
        updated.extend(changed);
        let images = render_epoch_timeline(&updated).unwrap();
        assert_eq!(&images[1].image.rgba[20..24], &[255, 255, 255, 255]);
        assert_eq!(&images[1].image.rgba[24..28], &[255, 255, 255, 255]);
    }

    #[test]
    fn timeline_rejects_unbounded_number_of_clear_frames() {
        let mut stream = Vec::new();
        for tick in 1..=17_u32 {
            let mut clear = packet(0x16, &[0, 2, 0, 1, 0x10, 0, 0, 0x80, 0, 0, 0]);
            clear[2..6].copy_from_slice(&(tick * 90_000).to_be_bytes());
            stream.extend(clear);
            stream.extend(packet(0x80, &[]));
        }
        assert!(render_self_contained_timeline(&stream).is_none());
    }

    #[test]
    fn transparent_and_translucent_overlay_preserves_color() {
        let mut pixel = [20, 30, 40, 255];
        over(&mut pixel, [255, 0, 0, 0]);
        assert_eq!(pixel, [20, 30, 40, 255]);
        over(&mut pixel, [255, 0, 0, 255]);
        assert_eq!(pixel, [255, 0, 0, 255]);
        over(&mut pixel, [0, 0, 255, 128]);
        assert!(pixel[0] >= 126 && pixel[0] <= 128);
        assert!(pixel[2] >= 127 && pixel[2] <= 129);
        assert_eq!(pixel[3], 255);
    }
}
