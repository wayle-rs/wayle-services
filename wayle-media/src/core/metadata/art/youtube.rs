//! Fallback artwork for sources that don't publish `mpris:artUrl`.
//!
//! Some MPRIS bridges (notably Firefox's web media session integration)
//! never populate `mpris:artUrl`, even though `xesam:url` points at a page
//! that clearly has a thumbnail (e.g. a YouTube watch page). When no art
//! URL is available, we derive one from a handful of well-known URL shapes.

/// Thumbnail qualities to try, best first. `maxresdefault` (up to 1280x720)
/// only exists for videos YouTube has generated it for -- most, but not
/// all -- so callers fall back down this list on download failure via
/// [`next_fallback`]. `hqdefault` (480x360) is generated for every video,
/// so the chain always terminates in a usable image.
const QUALITIES: [&str; 3] = ["maxresdefault", "sddefault", "hqdefault"];

/// Derives the best-quality thumbnail URL for known page URL shapes
/// (currently YouTube).
///
/// Returns `None` if `page_url` doesn't match a recognized pattern.
pub(crate) fn thumbnail_for_page_url(page_url: &str) -> Option<String> {
    let id = youtube_video_id(page_url)?;
    Some(thumbnail_url(&id, QUALITIES[0]))
}

/// Given a URL previously returned by [`thumbnail_for_page_url`] (or this
/// function), returns the next lower-quality candidate to try after a
/// download failure, or `None` once the chain is exhausted.
pub(crate) fn next_fallback(current_url: &str) -> Option<String> {
    let (id, quality) = parse_thumbnail_url(current_url)?;
    let idx = QUALITIES.iter().position(|q| *q == quality)?;
    let next_quality = QUALITIES.get(idx + 1)?;
    Some(thumbnail_url(&id, next_quality))
}

fn thumbnail_url(id: &str, quality: &str) -> String {
    format!("https://img.youtube.com/vi/{id}/{quality}.jpg")
}

fn parse_thumbnail_url(url: &str) -> Option<(&str, &'static str)> {
    let rest = url.strip_prefix("https://img.youtube.com/vi/")?;
    let (id, quality_jpg) = rest.split_once('/')?;
    let quality = quality_jpg.strip_suffix(".jpg")?;
    let matched = QUALITIES.iter().find(|q| **q == quality)?;
    Some((id, matched))
}

fn youtube_video_id(page_url: &str) -> Option<String> {
    let rest = page_url
        .strip_prefix("https://")
        .or_else(|| page_url.strip_prefix("http://"))?;
    let (host, path_and_query) = rest.split_once('/').unwrap_or((rest, ""));
    let host = host.strip_prefix("www.").unwrap_or(host);
    let (path, query) = path_and_query.split_once('?').unwrap_or((path_and_query, ""));

    match host {
        "youtube.com" | "m.youtube.com" | "youtube-nocookie.com" => match path {
            "watch" => query_param(query, "v").map(str::to_string),
            _ => path
                .strip_prefix("embed/")
                .or_else(|| path.strip_prefix("shorts/"))
                .map(|id| id.to_string()),
        },
        "youtu.be" if !path.is_empty() => Some(path.to_string()),
        _ => None,
    }
}

fn query_param<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key).then_some(v)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_id_from_watch_url() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn extracts_id_from_watch_url_with_extra_params() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=42s&list=PL1"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn extracts_id_from_short_url() {
        assert_eq!(
            youtube_video_id("https://youtu.be/dQw4w9WgXcQ"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn extracts_id_from_embed_url() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/embed/dQw4w9WgXcQ"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn extracts_id_from_shorts_url() {
        assert_eq!(
            youtube_video_id("https://www.youtube.com/shorts/dQw4w9WgXcQ"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn extracts_id_from_nocookie_domain() {
        assert_eq!(
            youtube_video_id("https://www.youtube-nocookie.com/embed/dQw4w9WgXcQ"),
            Some(String::from("dQw4w9WgXcQ"))
        );
    }

    #[test]
    fn rejects_non_youtube_url() {
        assert_eq!(
            youtube_video_id("https://example.com/watch?v=dQw4w9WgXcQ"),
            None
        );
    }

    #[test]
    fn rejects_youtube_home_page() {
        assert_eq!(youtube_video_id("https://www.youtube.com/"), None);
    }

    #[test]
    fn thumbnail_url_prefers_maxresdefault() {
        assert_eq!(
            thumbnail_for_page_url("https://youtu.be/dQw4w9WgXcQ"),
            Some(String::from(
                "https://img.youtube.com/vi/dQw4w9WgXcQ/maxresdefault.jpg"
            ))
        );
    }

    #[test]
    fn thumbnail_url_none_for_unrecognized_page() {
        assert_eq!(
            thumbnail_for_page_url("https://open.spotify.com/track/abc"),
            None
        );
    }

    #[test]
    fn fallback_chain_degrades_through_qualities() {
        let maxres = "https://img.youtube.com/vi/dQw4w9WgXcQ/maxresdefault.jpg";
        let sd = next_fallback(maxres).unwrap();
        assert_eq!(sd, "https://img.youtube.com/vi/dQw4w9WgXcQ/sddefault.jpg");

        let hq = next_fallback(&sd).unwrap();
        assert_eq!(hq, "https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg");

        assert_eq!(next_fallback(&hq), None);
    }

    #[test]
    fn fallback_chain_none_for_non_youtube_url() {
        assert_eq!(next_fallback("https://example.com/art.jpg"), None);
    }
}
