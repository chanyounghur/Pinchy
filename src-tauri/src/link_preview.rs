//! Fetch previews off the clipboard thread. No cookies, scripts, or remote images
//! enter the webview; redirects and image URLs obey the same network limits.
use crate::{
    clipboard,
    db::{Db, Item},
};
use reqwest::{blocking::Client, redirect::Policy, Url};
use scraper::{Html, Selector};
use std::{
    io::Read,
    net::{IpAddr, ToSocketAddrs},
    path::PathBuf,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

const HTML_LIMIT: usize = 1024 * 1024;
const IMAGE_LIMIT: usize = 5 * 1024 * 1024;

fn public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v) => {
            let [a, b, c, _] = v.octets();
            !(v.is_private()
                || v.is_loopback()
                || v.is_link_local()
                || v.is_broadcast()
                || v.is_documentation()
                || v.is_unspecified()
                || a == 0
                || a >= 224
                || (a == 100 && (64..=127).contains(&b))
                || (a == 198 && (b == 18 || b == 19))
                || (a == 192 && b == 0 && c == 0))
        }
        IpAddr::V6(v) => {
            if let Some(v4) = v.to_ipv4_mapped() {
                return public_ip(v4.into());
            }
            let s = v.segments();
            // Only global unicast, excluding special-use, documentation and 6to4.
            (s[0] & 0xe000) == 0x2000
                && !(s[0] == 0x2001 && s[1] < 0x200)
                && !(s[0] == 0x2001 && s[1] == 0xdb8)
                && s[0] != 0x2002
                && !(s[0] == 0x3fff && s[1] < 0x1000)
        }
    }
}

fn valid_url(url: &Url) -> bool {
    matches!(url.scheme(), "http" | "https")
        && url.username().is_empty()
        && url.password().is_none()
        && matches!(url.port_or_known_default(), Some(80 | 443))
}

fn fetch(mut url: Url, limit: usize) -> Option<(Url, Vec<u8>, String)> {
    for _ in 0..5 {
        if !valid_url(&url) {
            return None;
        }
        let host = url.host_str()?.trim_matches(['[', ']']);
        let addresses: Vec<_> = (host, url.port_or_known_default()?)
            .to_socket_addrs()
            .ok()?
            .collect();
        if addresses.is_empty() || addresses.iter().any(|a| !public_ip(a.ip())) {
            return None;
        }
        // Pin the checked DNS answer to prevent a second lookup changing the destination.
        let client = Client::builder()
            .no_proxy()
            .redirect(Policy::none())
            .timeout(Duration::from_secs(8))
            .connect_timeout(Duration::from_secs(4))
            .user_agent("Pinchy/0.3 (link preview)")
            .resolve_to_addrs(host, &addresses)
            .build()
            .ok()?;
        let response = client.get(url.clone()).send().ok()?;
        if response.status().is_redirection() {
            url = url
                .join(response.headers().get("location")?.to_str().ok()?)
                .ok()?;
            continue;
        }
        if !response.status().is_success()
            || response.content_length().is_some_and(|n| n > limit as u64)
        {
            return None;
        }
        let content_type = response
            .headers()
            .get("content-type")?
            .to_str()
            .ok()?
            .to_lowercase();
        let mut bytes = Vec::new();
        response
            .take(limit as u64 + 1)
            .read_to_end(&mut bytes)
            .ok()?;
        if bytes.len() > limit {
            return None;
        }
        return Some((url, bytes, content_type));
    }
    None
}

fn metadata(html: &str, base: &Url) -> (Option<String>, Option<Url>) {
    let doc = Html::parse_document(html);
    let selector = Selector::parse("meta").unwrap();
    let find = |key: &str| {
        doc.select(&selector).find_map(|node| {
            let name = node
                .value()
                .attr("property")
                .or_else(|| node.value().attr("name"))?;
            if !name.eq_ignore_ascii_case(key) {
                return None;
            }
            let value = node.value().attr("content")?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let title = find("og:title")
        .or_else(|| find("twitter:title"))
        .or_else(|| {
            doc.select(&Selector::parse("title").unwrap())
                .next()
                .map(|n| n.text().collect())
        })
        .map(|s: String| {
            s.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .chars()
                .take(300)
                .collect::<String>()
        })
        .filter(|s| !s.is_empty());
    let image = find("og:image:secure_url")
        .or_else(|| find("og:image"))
        .or_else(|| find("twitter:image"))
        .and_then(|s| base.join(&s).ok())
        .filter(valid_url);
    (title, image)
}

fn preview(item: &Item) -> (Option<String>, Option<Vec<u8>>) {
    let Some(url) = Url::parse(item.content.trim()).ok() else {
        return (None, None);
    };
    let Some((base, bytes, mime)) = fetch(url, HTML_LIMIT) else {
        return (None, None);
    };
    if !mime.starts_with("text/html") && !mime.starts_with("application/xhtml+xml") {
        return (None, None);
    }
    let (title, image) = metadata(&String::from_utf8_lossy(&bytes), &base);
    let image = image
        .and_then(|url| fetch(url, IMAGE_LIMIT))
        .and_then(|(_, bytes, mime)| {
            if !mime.starts_with("image/") {
                return None;
            }
            // Decode and re-encode a small raster: never persist active SVG/HTML.
            let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
                .with_guessed_format()
                .ok()?;
            let mut limits = image::Limits::default();
            limits.max_image_width = Some(8192);
            limits.max_image_height = Some(8192);
            limits.max_alloc = Some(64 * 1024 * 1024);
            reader.limits(limits);
            let image = reader.decode().ok()?.thumbnail(640, 360);
            let mut output = std::io::Cursor::new(Vec::new());
            image.write_to(&mut output, image::ImageFormat::Png).ok()?;
            Some(output.into_inner())
        });
    (title, image)
}

pub fn start(app: AppHandle, directory: PathBuf) {
    std::thread::Builder::new()
        .name("link-previews".into())
        .spawn(move || loop {
            let db = app.state::<Db>();
            let item = db.pending_link().ok().flatten();
            if let Some(item) = item {
                let (title, image) = preview(&item);
                let path = directory.join(format!("og-{}-{}.png", item.id, crate::db::now_ms()));
                if db
                    .save_link_preview(
                        &item,
                        title.as_deref(),
                        image.as_deref().map(|b| (path.as_path(), b)),
                    )
                    .is_ok()
                {
                    if title.is_some() || image.is_some() {
                        let _ = app.emit(clipboard::CHANGED_EVENT, ());
                    }
                }
            }
            std::thread::sleep(Duration::from_secs(2));
        })
        .expect("spawn link preview worker");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore = "requires public network access"]
    fn downloads_public_page_and_image() {
        let (url, bytes, mime) = fetch(
            Url::parse("https://github.com/chanyounghur/Pinchy").unwrap(),
            HTML_LIMIT,
        )
        .expect("HTML download");
        assert!(mime.starts_with("text/html"));
        let (title, image) = metadata(&String::from_utf8_lossy(&bytes), &url);
        assert!(title.is_some());
        let (_, bytes, mime) =
            fetch(image.expect("OG image URL"), IMAGE_LIMIT).expect("image download");
        assert!(mime.starts_with("image/"));
        assert!(image::load_from_memory(&bytes).is_ok());
    }

    #[test]
    fn parses_og_entities_relative_images_and_title_fallback() {
        let base = Url::parse("https://example.com/posts/one").unwrap();
        let (title, image) = metadata(
            r#"<title>Fallback</title><meta content=" Hello &amp; world " property="og:title"><meta property="og:image" content="../cover.png">"#,
            &base,
        );
        assert_eq!(title.as_deref(), Some("Hello & world"));
        assert_eq!(image.unwrap().as_str(), "https://example.com/cover.png");
        assert_eq!(
            metadata("<title> A\n  document </title>", &base)
                .0
                .as_deref(),
            Some("A document")
        );
        assert!(metadata(
            r#"<meta property="og:image" content="file:///secret">"#,
            &base
        )
        .1
        .is_none());
    }

    #[test]
    fn excludes_local_and_special_destinations() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "169.254.169.254",
            "192.168.1.1",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "2002:7f00:1::",
        ] {
            assert!(!public_ip(ip.parse().unwrap()), "{ip}");
        }
        assert!(public_ip("8.8.8.8".parse().unwrap()));
        assert!(public_ip("2606:4700:4700::1111".parse().unwrap()));
        for url in [
            "file:///tmp/x",
            "https://user:secret@example.com",
            "http://example.com:8080",
        ] {
            assert!(!valid_url(&Url::parse(url).unwrap()));
        }
    }
}
