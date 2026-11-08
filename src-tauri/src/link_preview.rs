//! Fetch previews off the clipboard thread. No cookies, scripts, or remote images
//! enter the webview; redirects, oEmbed and image URLs obey the same network limits.
use crate::{
    clipboard,
    db::Db,
};
use reqwest::{blocking::Client, header, redirect::Policy, Url};
use scraper::{Html, Selector};
use serde::Deserialize;
use std::{
    io::Read,
    net::{IpAddr, ToSocketAddrs},
    path::PathBuf,
    time::Duration,
};
use tauri::{AppHandle, Emitter, Manager};

/// Pages push their tags past a megabyte of inline script (YouTube), or
/// stream them into <body> (Next.js), so HTML is cut here, never refused.
const HTML_LIMIT: usize = 4 * 1024 * 1024;
const IMAGE_LIMIT: usize = 5 * 1024 * 1024;
const JSON_LIMIT: usize = 256 * 1024;
/// Image candidates tried before giving up on a picture.
const IMAGE_TRIES: usize = 3;
/// Site icons sit in a 36pt tile; this covers it at 2x and 3x.
const ICON_SIZE: u32 = 108;
const ICON_LIMIT: usize = 1024 * 1024;
const USER_AGENT: &str = concat!(
    "Pinchy/",
    env!("CARGO_PKG_VERSION"),
    " (link preview; +https://github.com/chanyounghur/Pinchy)"
);
const ACCEPT_HTML: &str = "text/html,application/xhtml+xml;q=0.9,*/*;q=0.8";
const ACCEPT_IMAGE: &str = "image/webp,image/png,image/jpeg,image/gif,image/*;q=0.5";
const ACCEPT_JSON: &str = "application/json";

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

struct Response {
    url: Url,
    bytes: Vec<u8>,
    mime: String,
}

/// `truncate` keeps the first `limit` bytes of a longer body instead of failing.
fn fetch(mut url: Url, accept: &str, limit: usize, truncate: bool) -> Option<Response> {
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
            .user_agent(USER_AGENT)
            .resolve_to_addrs(host, &addresses)
            .build()
            .ok()?;
        let response = client
            .get(url.clone())
            .header(header::ACCEPT, accept)
            .header(header::ACCEPT_LANGUAGE, "ko-KR,ko;q=0.9,en;q=0.8")
            .send()
            .ok()?;
        if response.status().is_redirection() {
            url = url
                .join(response.headers().get(header::LOCATION)?.to_str().ok()?)
                .ok()?;
            continue;
        }
        if !response.status().is_success()
            || (!truncate && response.content_length().is_some_and(|n| n > limit as u64))
        {
            return None;
        }
        let mime = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_lowercase();
        let bytes = read_body(response, limit, truncate)?;
        return Some(Response { url, bytes, mime });
    }
    None
}

fn read_body(body: impl Read, limit: usize, truncate: bool) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    body.take(limit as u64 + 1).read_to_end(&mut bytes).ok()?;
    if bytes.len() > limit {
        if !truncate {
            return None;
        }
        bytes.truncate(limit);
    }
    Some(bytes)
}

fn is_html(response: &Response) -> bool {
    let mime = &response.mime;
    mime.starts_with("text/html")
        || mime.starts_with("application/xhtml+xml")
        || (mime.is_empty() && response.bytes.trim_ascii_start().starts_with(b"<"))
}

/// Korean sites still serve EUC-KR, announced in the header or a <meta> tag.
fn decode(bytes: &[u8], mime: &str) -> String {
    let label = mime
        .split(';')
        .find_map(|p| p.trim().strip_prefix("charset="))
        .map(|s| s.trim_matches(['"', '\'']).to_string())
        .or_else(|| meta_charset(bytes));
    let encoding = label
        .and_then(|l| encoding_rs::Encoding::for_label(l.as_bytes()))
        .map(|e| e.output_encoding())
        .unwrap_or(encoding_rs::UTF_8);
    encoding.decode(bytes).0.into_owned()
}

/// Covers both `<meta charset=x>` and `<meta http-equiv content="...; charset=x">`.
fn meta_charset(bytes: &[u8]) -> Option<String> {
    let start = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_ascii_lowercase();
    let at = start.find("charset=")? + "charset=".len();
    let label: String = start[at..]
        .trim_start_matches(['"', '\''])
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | ':' | '.'))
        .collect();
    (!label.is_empty()).then_some(label)
}

fn clean_title(s: String) -> Option<String> {
    let s: String = s
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(300)
        .collect();
    (!s.is_empty()).then_some(s)
}

#[derive(Default)]
struct Meta {
    /// og:title or twitter:title, which beat an oEmbed title.
    title: Option<String>,
    /// The document <title>, used only when nothing better turns up.
    document_title: Option<String>,
    /// Image candidates, best first.
    images: Vec<Url>,
    /// Site icon candidates, largest first.
    icons: Vec<Url>,
    oembed: Option<Url>,
}

fn metadata(html: &str, base: &Url) -> Meta {
    let doc = Html::parse_document(html);
    let selector = |css: &str| Selector::parse(css).unwrap();
    let metas = selector("meta");
    let find = |key: &str| {
        doc.select(&metas).find_map(|node| {
            let e = node.value();
            let name = e
                .attr("property")
                .or_else(|| e.attr("name"))
                .or_else(|| e.attr("itemprop"))?;
            if !name.eq_ignore_ascii_case(key) {
                return None;
            }
            let value = e.attr("content")?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let link = |css: &str| {
        doc.select(&selector(css))
            .find_map(|n| n.value().attr("href"))
            .and_then(|href| base.join(href.trim()).ok())
            .filter(valid_url)
    };
    let mut images = Vec::new();
    let tagged = [
        "og:image:secure_url",
        "og:image",
        "og:image:url",
        "twitter:image",
        "twitter:image:src",
        "image",
    ]
    .into_iter()
    .filter_map(|key| find(key))
    .filter_map(|s| base.join(&s).ok());
    for url in tagged.chain(link(r#"link[rel~="image_src"]"#)) {
        if valid_url(&url) && !images.contains(&url) {
            images.push(url);
        }
    }
    Meta {
        title: find("og:title")
            .or_else(|| find("twitter:title"))
            .and_then(clean_title),
        document_title: doc
            .select(&selector("title"))
            .next()
            .map(|n| n.text().collect())
            .and_then(clean_title),
        images,
        icons: icons(&doc, base),
        oembed: link(r#"link[rel~="alternate"][type="application/json+oembed"]"#),
    }
}

/// Icons a page links, largest first: touch icons are usually 180px, and a
/// `sizes` hint orders the rest. SVG is left out since it is not decoded.
fn icons(doc: &Html, base: &Url) -> Vec<Url> {
    let links = Selector::parse("link[rel][href]").unwrap();
    let mut found: Vec<(u32, Url)> = doc
        .select(&links)
        .filter_map(|n| {
            let e = n.value();
            let rel = e.attr("rel")?.to_ascii_lowercase();
            let touch = rel.split_whitespace().any(|r| r.starts_with("apple-touch-icon"));
            if !touch && !rel.split_whitespace().any(|r| r == "icon") {
                return None;
            }
            if e.attr("type").is_some_and(|t| t.contains("svg")) || e.attr("sizes").is_some_and(|s| s.eq_ignore_ascii_case("any")) {
                return None;
            }
            let url = base.join(e.attr("href")?.trim()).ok().filter(valid_url)?;
            if url.path().to_ascii_lowercase().ends_with(".svg") {
                return None;
            }
            let size = e
                .attr("sizes")
                .and_then(|s| s.split_whitespace().filter_map(|s| s.split(['x', 'X']).next()?.parse().ok()).max())
                .unwrap_or(if touch { 180 } else { 16 });
            Some((size, url))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let mut icons: Vec<Url> = Vec::new();
    for (_, url) in found {
        if !icons.contains(&url) {
            icons.push(url);
        }
    }
    icons
}

/// Where browsers look when a page names no icon, or could not be read.
fn default_icons(url: &Url) -> Vec<Url> {
    ["/apple-touch-icon.png", "/favicon.ico"].iter().filter_map(|p| url.join(p).ok()).collect()
}

/// Naver blog posts sit in a frame whose outer page carries no tags; the
/// mobile page has them.
fn mobile_twin(mut url: Url) -> Url {
    if url.host_str() == Some("blog.naver.com") {
        let _ = url.set_host(Some("m.blog.naver.com"));
    }
    url
}

/// Official oEmbed endpoints for sites whose pages are script shells or turn
/// automated requests away, used when the page does not name its own.
fn known_oembed(url: &Url) -> Option<Url> {
    let host = url.host_str()?;
    let host = host.strip_prefix("www.").unwrap_or(host);
    let host = host.strip_prefix("m.").unwrap_or(host);
    let endpoint = match host {
        "youtube.com" | "youtu.be" | "music.youtube.com" => "https://www.youtube.com/oembed",
        "tiktok.com" => "https://www.tiktok.com/oembed",
        "vimeo.com" => "https://vimeo.com/api/oembed.json",
        "open.spotify.com" => "https://open.spotify.com/oembed",
        "soundcloud.com" => "https://soundcloud.com/oembed",
        "flickr.com" => "https://www.flickr.com/services/oembed",
        "reddit.com" => "https://www.reddit.com/oembed",
        "x.com" | "twitter.com" | "mobile.twitter.com" => "https://publish.x.com/oembed",
        _ => return None,
    };
    let mut endpoint = Url::parse(endpoint).ok()?;
    endpoint
        .query_pairs_mut()
        .append_pair("url", url.as_str())
        .append_pair("format", "json");
    Some(endpoint)
}

#[derive(Deserialize)]
struct OEmbed {
    title: Option<String>,
    author_name: Option<String>,
    /// Embed markup; a post's text when the provider gives no title (X).
    html: Option<String>,
    thumbnail_url: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    url: Option<String>,
}

fn oembed(endpoint: Url) -> Option<(Option<String>, Option<Url>)> {
    let response = fetch(endpoint, ACCEPT_JSON, JSON_LIMIT, false)?;
    let data: OEmbed = serde_json::from_slice(&response.bytes).ok()?;
    let photo = data.kind.as_deref() == Some("photo");
    let image = data
        .thumbnail_url
        .or(data.url.filter(|_| photo))
        .and_then(|s| response.url.join(&s).ok())
        .filter(valid_url);
    let title = data.title.and_then(clean_title).or_else(|| {
        let text = data.html.as_deref().and_then(embed_text)?;
        match data.author_name.and_then(clean_title) {
            Some(author) => clean_title(format!("{author}: {text}")),
            None => Some(text),
        }
    });
    Some((title, image))
}

/// The first paragraph of oEmbed markup, as plain text.
fn embed_text(html: &str) -> Option<String> {
    let doc = Html::parse_fragment(html);
    let p = doc.select(&Selector::parse("p").unwrap()).next()?;
    clean_title(p.text().collect())
}

fn thumbnail(url: Url) -> Option<Vec<u8>> {
    raster(url, IMAGE_LIMIT, 640, 360, 1)
}

/// A site icon, kept only when it is at least 32px so a 16px favicon is not
/// blown up into a blur.
fn site_icon(url: Url) -> Option<Vec<u8>> {
    raster(url, ICON_LIMIT, ICON_SIZE, ICON_SIZE, 32)
}

fn raster(url: Url, limit: usize, width: u32, height: u32, min_side: u32) -> Option<Vec<u8>> {
    let response = fetch(url, ACCEPT_IMAGE, limit, false)?;
    // Decode and re-encode a small raster: never persist active SVG/HTML. The
    // decoder sniffs the bytes, so a CDN's wrong content type does not matter.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(response.bytes))
        .with_guessed_format()
        .ok()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(8192);
    limits.max_image_height = Some(8192);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let image = reader.decode().ok()?;
    if image.width().min(image.height()) < min_side {
        return None;
    }
    let image = image.thumbnail(width, height);
    let mut output = std::io::Cursor::new(Vec::new());
    image.write_to(&mut output, image::ImageFormat::Png).ok()?;
    Some(output.into_inner())
}

/// oEmbed speaks first for the providers built in above, whose page tags can
/// be generic; elsewhere it only fills what the tags lack. The bare <title>
/// comes last.
#[derive(Default)]
struct Preview {
    title: Option<String>,
    image: Option<Vec<u8>>,
    /// Fetched only when there is no image to show.
    icon: Option<Vec<u8>>,
}

fn preview(link: &str) -> Preview {
    let Some(url) = Url::parse(link.trim()).ok().map(mobile_twin) else {
        return Preview::default();
    };
    let meta = fetch(url.clone(), ACCEPT_HTML, HTML_LIMIT, true)
        .filter(is_html)
        .map(|page| metadata(&decode(&page.bytes, &page.mime), &page.url))
        .unwrap_or_default();
    let known = known_oembed(&url);
    let provider = known.is_some();
    let (oembed_title, oembed_image) = if provider || meta.title.is_none() || meta.images.is_empty() {
        meta.oembed.clone().or(known).and_then(oembed).unwrap_or_default()
    } else {
        (None, None)
    };
    let (title, images): (_, Vec<_>) = if provider {
        (oembed_title.or(meta.title), oembed_image.into_iter().chain(meta.images).collect())
    } else {
        (meta.title.or(oembed_title), meta.images.into_iter().chain(oembed_image).collect())
    };
    let image = images.into_iter().take(IMAGE_TRIES).find_map(thumbnail);
    let icon = if image.is_none() {
        let mut candidates = meta.icons;
        for url in default_icons(&url) {
            if !candidates.contains(&url) {
                candidates.push(url);
            }
        }
        candidates.into_iter().take(IMAGE_TRIES).find_map(site_icon)
    } else {
        None
    };
    Preview { title: title.or(meta.document_title), image, icon }
}

pub fn start(app: AppHandle, directory: PathBuf) {
    std::thread::Builder::new()
        .name("link-previews".into())
        .spawn(move || loop {
            let db = app.state::<Db>();
            let item = db.pending_link().ok().flatten();
            if let Some(item) = item {
                let Preview { title, image, icon } = preview(&item.content);
                let stamp = format!("{}-{}", item.id, crate::db::now_ms());
                let path = directory.join(format!("og-{stamp}.png"));
                let icon_path = directory.join(format!("og-icon-{stamp}.png"));
                if db
                    .save_link_preview(
                        &item,
                        title.as_deref(),
                        image.as_deref().map(|b| (path.as_path(), b)),
                        icon.as_deref().map(|b| (icon_path.as_path(), b)),
                    )
                    .is_ok()
                {
                    if title.is_some() || image.is_some() || icon.is_some() {
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
        let page = fetch(
            Url::parse("https://github.com/chanyounghur/Pinchy").unwrap(),
            ACCEPT_HTML,
            HTML_LIMIT,
            true,
        )
        .expect("HTML download");
        assert!(is_html(&page));
        let meta = metadata(&decode(&page.bytes, &page.mime), &page.url);
        assert!(meta.title.is_some());
        let image = meta.images.into_iter().next().expect("OG image URL");
        assert!(image::load_from_memory(&thumbnail(image).expect("image download")).is_ok());
    }

    /// Prints what each URL in `PINCHY_PROBE` (whitespace separated) previews as:
    /// `PINCHY_PROBE="https://a https://b" cargo test probe -- --ignored --nocapture`
    #[test]
    #[ignore = "requires public network access"]
    fn probe() {
        let urls = std::env::var("PINCHY_PROBE").unwrap_or_default();
        let urls: Vec<String> = urls.split_whitespace().map(String::from).collect();
        let handles: Vec<_> = urls
            .into_iter()
            .map(|url| {
                std::thread::spawn(move || {
                    let started = std::time::Instant::now();
                    let Preview { title, image, icon } = preview(&url);
                    let size = |b: Option<Vec<u8>>| b.and_then(|b| image::load_from_memory(&b).ok()).map(|i| format!("{}x{}", i.width(), i.height()));
                    let (image, icon) = (size(image), size(icon));
                    format!("{:>5}ms {url}\n        title={title:?} image={image:?} icon={icon:?}", started.elapsed().as_millis())
                })
            })
            .collect();
        for h in handles {
            println!("{}", h.join().unwrap());
        }
    }

    #[test]
    fn parses_og_entities_relative_images_and_title_fallback() {
        let base = Url::parse("https://example.com/posts/one").unwrap();
        let meta = metadata(
            r#"<title>Fallback</title><meta content=" Hello &amp; world " property="og:title"><meta property="og:image" content="../cover.png">"#,
            &base,
        );
        assert_eq!(meta.title.as_deref(), Some("Hello & world"));
        assert_eq!(meta.images[0].as_str(), "https://example.com/cover.png");
        let meta = metadata("<title> A\n  document </title>", &base);
        assert_eq!(meta.title, None);
        assert_eq!(meta.document_title.as_deref(), Some("A document"));
        assert!(metadata(
            r#"<meta property="og:image" content="file:///secret">"#,
            &base
        )
        .images
        .is_empty());
    }

    #[test]
    fn collects_image_fallbacks_in_order_and_finds_oembed() {
        let base = Url::parse("https://example.com/a").unwrap();
        let meta = metadata(
            r#"<link rel="image_src" href="/src.png">
            <meta itemprop="image" content="/item.png">
            <meta name="twitter:image" content="/tw.png">
            <meta property="og:image" content="/og.png">
            <meta property="og:image:url" content="/og.png">
            <link rel="alternate" type="application/json+oembed" href="/oembed?u=a">"#,
            &base,
        );
        let images: Vec<_> = meta.images.iter().map(|u| u.path()).collect();
        assert_eq!(images, ["/og.png", "/tw.png", "/item.png", "/src.png"]);
        assert_eq!(meta.oembed.unwrap().as_str(), "https://example.com/oembed?u=a");
    }

    #[test]
    fn orders_site_icons_largest_first_and_skips_svg() {
        let base = Url::parse("https://example.com/a/b").unwrap();
        let meta = metadata(
            r#"<link rel="icon" href="/16.png">
            <link rel="icon" type="image/svg+xml" href="/logo.svg">
            <link rel="shortcut icon" href="/f.ico">
            <link rel="icon" sizes="32x32 192x192" href="/192.png">
            <link rel="apple-touch-icon" href="touch.png">
            <link rel="apple-touch-icon-precomposed" sizes="57x57" href="/57.png">
            <link rel="stylesheet" href="/s.css">"#,
            &base,
        );
        let icons: Vec<_> = meta.icons.iter().map(|u| u.path()).collect();
        assert_eq!(icons, ["/192.png", "/a/touch.png", "/57.png", "/16.png", "/f.ico"]);
        let fallback: Vec<_> = default_icons(&base).iter().map(|u| u.to_string()).collect();
        assert_eq!(fallback, ["https://example.com/apple-touch-icon.png", "https://example.com/favicon.ico"]);
    }

    #[test]
    fn titles_a_post_from_its_embed_text() {
        let html = r#"<blockquote class="twitter-tweet"><p lang="en" dir="ltr">just setting up my
            <a href="https://t.co/x">twttr</a></p>&mdash; jack (@jack) <a href="https://x.com/jack/status/20">March 21, 2006</a></blockquote><script src="https://platform.x.com/widgets.js"></script>"#;
        assert_eq!(embed_text(html).as_deref(), Some("just setting up my twttr"));
        assert_eq!(embed_text("<blockquote><a href='/'>only a link</a></blockquote>"), None);
        assert!(known_oembed(&Url::parse("https://twitter.com/jack/status/20").unwrap()).is_some());
    }

    #[test]
    fn decodes_euc_kr_from_header_or_meta() {
        let (bytes, _, _) = encoding_rs::EUC_KR.encode("<title>뽐뿌</title>");
        assert!(decode(&bytes, "text/html; charset=euc-kr").contains("뽐뿌"));
        let mut page = br#"<meta http-equiv="Content-Type" content="text/html; charset=EUC-KR">"#.to_vec();
        page.extend_from_slice(&bytes);
        assert!(decode(&page, "text/html").contains("뽐뿌"));
        let mut page = b"<meta charset='euc-kr'>".to_vec();
        page.extend_from_slice(&bytes);
        assert!(decode(&page, "").contains("뽐뿌"));
        assert!(decode("<title>토스</title>".as_bytes(), "text/html").contains("토스"));
    }

    #[test]
    fn cuts_long_html_but_refuses_other_long_bodies() {
        let page = vec![b'x'; 5000];
        assert_eq!(read_body(page.as_slice(), 1000, true).unwrap().len(), 1000);
        assert!(read_body(page.as_slice(), 1000, false).is_none());
        assert_eq!(read_body(page.as_slice(), 5000, false).unwrap().len(), 5000);
    }

    #[test]
    fn rewrites_framed_pages_and_knows_oembed_providers() {
        let twin = mobile_twin(Url::parse("https://blog.naver.com/naver_diary/223").unwrap());
        assert_eq!(twin.as_str(), "https://m.blog.naver.com/naver_diary/223");
        let endpoint = known_oembed(&Url::parse("https://youtu.be/dQw4w9WgXcQ").unwrap()).unwrap();
        assert_eq!(endpoint.host_str(), Some("www.youtube.com"));
        assert!(endpoint.query().unwrap().contains("url=https%3A%2F%2Fyoutu.be%2FdQw4w9WgXcQ"));
        assert!(known_oembed(&Url::parse("https://m.youtube.com/watch?v=1").unwrap()).is_some());
        assert!(known_oembed(&Url::parse("https://example.com/").unwrap()).is_none());
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
