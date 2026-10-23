//! Conservative local TV-episode naming hints from indexed relative paths.
//! This is not a Jellyfin Emby.Naming port and must not claim complete
//! classification, online metadata, multiple episodes or arbitrary layouts.
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EpisodeHint {
    pub series_name: String,
    pub season: u16,
    pub episode: u16,
    pub episode_title: Option<String>,
}

fn season_folder_number(name: &str) -> Option<u16> {
    let lower = name.trim().to_ascii_lowercase();
    let digits = lower
        .strip_prefix("season ")
        .or_else(|| lower.strip_prefix("season_"))
        .or_else(|| lower.strip_prefix('s'))?;
    if digits.is_empty() || digits.len() > 3 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

/// Return a complete SxxEyy token with byte end offset.
/// ASCII-only scanning is safe with UTF-8 filenames: token boundaries are
/// always ASCII bytes, not interior UTF-8 continuation bytes.
fn episode_token(stem: &str) -> Option<(u16, u16, usize)> {
    let bytes = stem.as_bytes();
    for i in 0..bytes.len() {
        if !bytes[i].eq_ignore_ascii_case(&b's') || i > 0 && bytes[i - 1].is_ascii_alphanumeric() {
            continue;
        }
        let season_start = i + 1;
        let mut j = season_start;
        while j < bytes.len() && j - season_start < 3 && bytes[j].is_ascii_digit() {
            j += 1;
        }
        if j == season_start || j >= bytes.len() || !bytes[j].eq_ignore_ascii_case(&b'e') {
            continue;
        }
        let episode_start = j + 1;
        let mut k = episode_start;
        while k < bytes.len() && k - episode_start < 3 && bytes[k].is_ascii_digit() {
            k += 1;
        }
        if k == episode_start || k < bytes.len() && bytes[k].is_ascii_alphanumeric() {
            continue;
        }
        let season = stem[season_start..j].parse().ok()?;
        let episode = stem[episode_start..k].parse().ok()?;
        return Some((season, episode, k));
    }
    None
}

/// Only identify an episode when the season directory and filename agree.
/// Other video files remain generic video items until richer naming support.
pub fn episode_hint(relative_path: &Path) -> Option<EpisodeHint> {
    let stem = relative_path.file_stem()?.to_str()?;
    let parent = relative_path.parent()?;
    let season_folder = parent.file_name()?.to_str()?;
    let series_name = parent.parent()?.file_name()?.to_str()?.trim();
    if series_name.is_empty() {
        return None;
    }
    let folder_season = season_folder_number(season_folder)?;
    let (season, episode, end) = episode_token(stem)?;
    if folder_season != season || episode_token(&stem[end..]).is_some() {
        // Multi-episode filenames are not single episode identifiers.
        return None;
    }
    let title =
        stem[end..].trim_matches(|c: char| c.is_whitespace() || matches!(c, '-' | '_' | '.'));
    Some(EpisodeHint {
        series_name: series_name.to_owned(),
        season,
        episode,
        episode_title: (!title.is_empty()).then(|| title.to_owned()),
    })
}

/// Only infer a movie from a matching Title (YYYY) directory and file stem.
/// This rejects movie-looking files dropped into an arbitrary mixed library
/// and makes no genre, edition or provider metadata claims.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieHint {
    pub title: String,
    pub production_year: u16,
}

pub fn movie_hint(relative_path: &Path) -> Option<MovieHint> {
    let stem = relative_path.file_stem()?.to_str()?.trim();
    let folder = relative_path.parent()?.file_name()?.to_str()?.trim();
    if !stem.eq_ignore_ascii_case(folder) {
        return None;
    }
    let without_paren = stem.strip_suffix(')')?;
    let (title, year_text) = without_paren.rsplit_once(" (")?;
    if title.trim().is_empty()
        || year_text.len() != 4
        || !year_text.bytes().all(|byte| byte.is_ascii_digit())
    {
        return None;
    }
    let production_year: u16 = year_text.parse().ok()?;
    if !(1888..=2100).contains(&production_year) {
        return None;
    }
    Some(MovieHint {
        title: title.trim().to_owned(),
        production_year,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_single_episodes_in_consistent_series_season_folders() {
        let item = episode_hint(Path::new(
            "The Signal/Season 01/The Signal - S01E02 - Arrival.mkv",
        ))
        .unwrap();
        assert_eq!(
            item,
            EpisodeHint {
                series_name: "The Signal".to_owned(),
                season: 1,
                episode: 2,
                episode_title: Some("Arrival".to_owned()),
            }
        );
        let special = episode_hint(Path::new("Nova/S00/Nova.s00e01.mp4")).unwrap();
        assert_eq!(special.season, 0);
        assert_eq!(special.episode, 1);
        assert_eq!(special.episode_title, None);
    }

    #[test]
    fn refuses_ambiguous_or_mismatched_episode_naming() {
        for path in [
            "Unsorted/S01E01.mkv",
            "Show/Season 2/Show - S01E01.mkv",
            "Show/Season 01/Show - XS01E01.mkv",
            "Show/Season 01/Show - S01E01E02.mkv",
            "Show/Season 01/Show - S01E1234.mkv",
            "Show/Season 01/Feature Film.mkv",
            "Show/Season 01/Show - S01E01Part1.mkv",
            "Show/Season 01/Show - S01E01 - S01E02.mkv",
            "Show/Season 01/Show - S01E01 S01E03.mkv",
            "Show/Season 01/Show - S01E01.mp4/other.mkv",
        ] {
            assert!(episode_hint(Path::new(path)).is_none(), "{path}");
        }
    }

    #[test]
    fn recognizes_only_matching_movie_folder_and_file_pairs() {
        assert_eq!(
            movie_hint(Path::new(
                "Films/Café de México (2024)/Café de México (2024).mkv"
            )),
            Some(MovieHint {
                title: "Café de México".to_owned(),
                production_year: 2024,
            })
        );
        assert_eq!(
            movie_hint(Path::new("Arrival (2016)/arrival (2016).mp4"))
                .unwrap()
                .title,
            "arrival"
        );
        for path in [
            "Feature (2021).mkv",
            "Shows/Season 01/Film (2021).mkv",
            "Different (2021)/Film (2021).mkv",
            "Film (abcd)/Film (abcd).mp4",
            "Film (1776)/Film (1776).mp4",
            "Film (2101)/Film (2101).mp4",
            "Film (2021)/Film (2021) - Extended.mp4",
            "Film (2021)/Film (2021).English.srt",
        ] {
            assert!(movie_hint(Path::new(path)).is_none(), "{path}");
        }
    }

    #[test]
    fn supports_unicode_series_names_without_splitting_utf8_bytes() {
        let item = episode_hint(Path::new(
            "Café de México/Season 3/Café de México - S03E08 - El viaje.mkv",
        ))
        .unwrap();
        assert_eq!(item.series_name, "Café de México");
        assert_eq!(item.episode_title.as_deref(), Some("El viaje"));
    }
}
