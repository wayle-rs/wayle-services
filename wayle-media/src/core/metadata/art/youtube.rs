//! Fallback artwork for sources that don't publish `mpris:artUrl`.
//!
//! Some MPRIS bridges (notably Firefox's web media session integration)
//! never populate `mpris:artUrl`, even though `xesam:url` points at a page
//! that clearly has a thumbnail (e.g. a YouTube watch page). When no art
//! URL is available, we derive one from a handful of well-known URL shapes.

const THUMBNAIL_QUALITY: &str = "hqdefault";

/// Derives a thumbnail URL for known page URL shapes (currently YouTube).
///
/// Returns `None` if `page_url` doesn't match a recognized pattern.
pub(crate) fn thumbnail_for_page_url(page_url: &str) -> Option<String> {
    let id = youtube_video_id(page_url)?;
    Some(format!(
        "https://img.youtube.com/vi/{id}/{THUMBNAIL_QUALITY}.jpg"
    ))
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
    fn thumbnail_url_uses_hqdefault() {
        assert_eq!(
            thumbnail_for_page_url("https://youtu.be/dQw4w9WgXcQ"),
            Some(String::from(
                "https://img.youtube.com/vi/dQw4w9WgXcQ/hqdefault.jpg"
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
}
