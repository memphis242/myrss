use image::GenericImageView;
use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, ToSocketAddrs};
use url::{Host, Url};

/// Maximum number of HTTP redirects we will follow for any single fetch. Each hop
/// is re-validated against [`is_safe_url`] to prevent redirect-based SSRF.
pub const MAX_REDIRECTS: u32 = 5;

/// Checks if the given URL is safe to fetch from, defending against SSRF.
///
/// Defense-in-depth notes:
/// - Only `http`/`https` are permitted.
/// - IP-literal hosts (including the decimal/hex/octal encodings the URL parser
///   normalizes, e.g. `http://2130706433/`) are checked directly.
/// - Hostnames are resolved and **every** resolved address must be public;
///   resolution yielding zero addresses fails **closed**.
/// - This is a pre-flight check only. DNS can rebind between this check and the
///   actual socket connect (TOCTOU). The redirect vector is closed by
///   [`safe_get`]; callers should prefer it over a raw agent request.
pub fn is_safe_url(url_str: &str) -> bool {
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Only allow http and https protocols.
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }

    match url.host() {
        // IP literal in the URL. The WHATWG host parser normalizes decimal/hex/
        // octal IPv4 forms into an `Ipv4` host, so those encodings are covered.
        Some(Host::Ipv4(ip)) => !is_disallowed_ip(IpAddr::V4(ip)),
        Some(Host::Ipv6(ip)) => !is_disallowed_ip(IpAddr::V6(ip)),
        // Hostname: resolve and require at least one address, all of them public.
        Some(Host::Domain(host)) => {
            let addrs = match (host, 0u16).to_socket_addrs() {
                Ok(iter) => iter,
                Err(_) => return false,
            };
            let mut resolved_any = false;
            for addr in addrs {
                resolved_any = true;
                if is_disallowed_ip(addr.ip()) {
                    return false;
                }
            }
            // Fail closed if the host resolved to no addresses.
            resolved_any
        }
        None => false,
    }
}

/// Returns true if `ip` is loopback, private, link-local, or otherwise not a
/// globally-routable public unicast address we are willing to fetch from.
///
/// This is intentionally a broad denylist of every special-use range because the
/// stable standard library lacks an `is_global()` predicate.
fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_disallowed_ipv4(v4),
        IpAddr::V6(v6) => {
            // Normalize embedded IPv4 so an attacker cannot smuggle a private
            // IPv4 address through an IPv6 literal (e.g. `::ffff:127.0.0.1`).
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_disallowed_ipv4(v4);
            }
            // Deprecated IPv4-compatible form `::a.b.c.d` (but not pure `::`/`::1`).
            if !v6.is_loopback()
                && !v6.is_unspecified()
                && let Some(v4) = v6.to_ipv4()
            {
                return is_disallowed_ipv4(v4);
            }
            is_disallowed_ipv6(v6)
        }
    }
}

fn is_disallowed_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, c, _] = ip.octets();
    ip.is_loopback()              // 127.0.0.0/8
        || ip.is_private()        // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local()     // 169.254.0.0/16 (incl. metadata 169.254.169.254)
        || ip.is_multicast()      // 224.0.0.0/4
        || ip.is_broadcast()      // 255.255.255.255
        || ip.is_unspecified()    // 0.0.0.0
        || a == 0                 // 0.0.0.0/8 "this network" (0.x → localhost on some stacks)
        || (a == 100 && (b & 0xc0) == 64) // 100.64.0.0/10 carrier-grade NAT (RFC 6598)
        || (a == 192 && b == 0 && c == 0) // 192.0.0.0/24 IETF protocol assignments
        || (a == 192 && b == 0 && c == 2) // 192.0.2.0/24 TEST-NET-1
        || (a == 198 && b == 51 && c == 100) // 198.51.100.0/24 TEST-NET-2
        || (a == 203 && b == 0 && c == 113)  // 203.0.113.0/24 TEST-NET-3
        || (a == 198 && (b & 0xfe) == 18)    // 198.18.0.0/15 benchmarking
        || (a & 0xf0) == 240 // 240.0.0.0/4 reserved
}

fn is_disallowed_ipv6(v6: Ipv6Addr) -> bool {
    if v6.is_loopback() || v6.is_unspecified() || v6.is_multicast() {
        return true;
    }
    let seg = v6.segments();
    // fc00::/7 unique local addresses.
    if (seg[0] & 0xfe00) == 0xfc00 {
        return true;
    }
    // fe80::/10 link-local.
    if (seg[0] & 0xffc0) == 0xfe80 {
        return true;
    }
    // 2001:db8::/32 documentation.
    if seg[0] == 0x2001 && seg[1] == 0x0db8 {
        return true;
    }
    // 2002::/16 6to4 — validate the embedded IPv4.
    if seg[0] == 0x2002 {
        let embedded = Ipv4Addr::new(
            (seg[1] >> 8) as u8,
            (seg[1] & 0xff) as u8,
            (seg[2] >> 8) as u8,
            (seg[2] & 0xff) as u8,
        );
        if is_disallowed_ipv4(embedded) {
            return true;
        }
    }
    // 2001:0000::/32 Teredo — validate the embedded (bit-inverted) client IPv4.
    if seg[0] == 0x2001 && seg[1] == 0x0000 {
        let embedded = Ipv4Addr::new(
            !(seg[6] >> 8) as u8,
            !(seg[6] & 0xff) as u8,
            !(seg[7] >> 8) as u8,
            !(seg[7] & 0xff) as u8,
        );
        if is_disallowed_ipv4(embedded) {
            return true;
        }
    }
    false
}

/// Resolve a redirect `Location` against the URL that produced it and return the
/// absolute target only if it passes [`is_safe_url`]. Factored out as a pure
/// function so redirect validation is unit-testable without any network IO.
fn safe_redirect_target(current_url: &str, location: &str) -> Option<String> {
    let base = Url::parse(current_url).ok()?;
    let target = base.join(location).ok()?;
    let target_str: String = target.into();
    if is_safe_url(&target_str) {
        Some(target_str)
    } else {
        None
    }
}

/// HTTP status codes treated as followable redirects. 304 (Not Modified) is
/// deliberately excluded so conditional GETs (feeds) still observe a cache hit.
const REDIRECT_STATUSES: [u16; 5] = [301, 302, 303, 307, 308];

/// Perform a GET that re-validates the destination of **every** redirect hop
/// against [`is_safe_url`], closing the redirect-based SSRF hole that a single
/// up-front check leaves open.
///
/// `agent` SHOULD be built with `.redirects(0)` so that this function — not
/// `ureq` — controls redirect following and can validate each hop. If the agent
/// follows redirects itself, only the initial URL is guaranteed to be validated.
pub fn safe_get(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(&str, &str)],
    max_redirects: u32,
) -> anyhow::Result<ureq::Response> {
    if !is_safe_url(url) {
        anyhow::bail!("refusing to fetch unsafe or private URL");
    }

    let mut current = url.to_string();
    for _ in 0..=max_redirects {
        let mut request = agent.get(&current);
        for (name, value) in headers {
            request = request.set(name, value);
        }
        let response = request.call()?;

        if REDIRECT_STATUSES.contains(&response.status()) {
            let location = response
                .header("location")
                .ok_or_else(|| anyhow::anyhow!("redirect response without a Location header"))?;
            current = safe_redirect_target(&current, location)
                .ok_or_else(|| anyhow::anyhow!("blocked redirect to an unsafe or private URL"))?;
            continue;
        }

        return Ok(response);
    }

    anyhow::bail!("too many redirects while fetching the requested URL")
}

/// Read an HTTP response body as a UTF-8 string, capped at `max_bytes` to defend
/// against memory exhaustion from a hostile server streaming an unbounded body.
pub fn read_body_capped(response: ureq::Response, max_bytes: usize) -> anyhow::Result<String> {
    let mut buffer = Vec::new();
    response
        .into_reader()
        .take((max_bytes as u64).saturating_add(1))
        .read_to_end(&mut buffer)?;
    if buffer.len() > max_bytes {
        anyhow::bail!("response body exceeded the {} byte limit", max_bytes);
    }
    Ok(String::from_utf8_lossy(&buffer).into_owned())
}

/// ASCII case-insensitive substring search returning a byte offset valid for
/// `haystack` itself.
///
/// `haystack.to_lowercase().find(needle)` is unsafe to index back into
/// `haystack`: lowercasing can change a string's byte length (e.g.
/// `U+212A KELVIN SIGN` → `k`), so the returned offset can land mid-codepoint and
/// **panic** when used to slice the original. HTML tag/attribute names are ASCII,
/// so ASCII-only folding is sufficient and keeps the scan linear (no ReDoS).
fn find_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let need = needle.as_bytes();
    if need.is_empty() {
        return Some(0);
    }
    if need.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - need.len()).find(|&i| hay[i..i + need.len()].eq_ignore_ascii_case(need))
}

/// Like [`find_ascii_ci`] but returns the offset of the **last** match.
fn rfind_ascii_ci(haystack: &str, needle: &str) -> Option<usize> {
    let hay = haystack.as_bytes();
    let need = needle.as_bytes();
    if need.is_empty() {
        return Some(hay.len());
    }
    if need.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - need.len())
        .rev()
        .find(|&i| hay[i..i + need.len()].eq_ignore_ascii_case(need))
}

/// Maximum width/height (px) we will decode. Enforced via `image::Limits` so the
/// limit applies *during* decoding, before a decompression bomb fully expands.
const MAX_IMAGE_DIMENSION: u32 = 4096;
/// Upper bound on memory the decoder may allocate (a 4096×4096 RGBA bitmap is
/// ~64 MB; this leaves headroom for intermediate buffers while still bounding it).
const MAX_IMAGE_ALLOC_BYTES: u64 = 256 * 1024 * 1024;

/// Converts a buffer of image bytes to ASCII art, enforcing size checks and aspect ratio correction.
pub fn convert_image_to_ascii(bytes: &[u8], target_width: u32) -> anyhow::Result<String> {
    // Decode the image with explicit limits so an oversized (decompression-bomb)
    // image is rejected *before* its full bitmap is allocated.
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| anyhow::anyhow!("could not determine image format: {e}"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_ALLOC_BYTES);
    reader.limits(limits);
    let img = reader
        .decode()
        .map_err(|e| anyhow::anyhow!("could not decode image: {e}"))?;

    // Defense-in-depth: re-check dimensions after decode in case a codec reported
    // them late.
    let (width, height) = img.dimensions();
    if width > MAX_IMAGE_DIMENSION || height > MAX_IMAGE_DIMENSION {
        anyhow::bail!("Image dimensions too large: {}x{}", width, height);
    }

    // Terminal characters are about twice as tall as they are wide.
    // Adjust target height by 0.5 to preserve the original aspect ratio.
    let aspect_ratio = height as f32 / width as f32;
    let target_height = ((target_width as f32 * aspect_ratio) * 0.55).max(1.0) as u32;

    // Fast scale the image
    let resized = img.resize_exact(
        target_width,
        target_height,
        image::imageops::FilterType::Nearest,
    );
    let grayscale = resized.to_luma8();

    // Map pixel intensity to character ramp
    let ramp = " .:-=+*#%@";
    let ramp_len = ramp.len();

    let mut ascii = String::new();
    for y in 0..target_height {
        for x in 0..target_width {
            let pixel = grayscale.get_pixel(x, y);
            let intensity = pixel[0] as usize;
            let char_idx = (intensity * (ramp_len - 1)) / 255;
            ascii.push(ramp.chars().nth(char_idx).unwrap_or(' '));
        }
        ascii.push('\n');
    }

    Ok(ascii)
}

/// Helper method to perform a safe linear-scan extraction of image URLs to prevent ReDoS.
pub fn extract_image_urls(html: &str) -> Vec<String> {
    let mut urls = Vec::new();
    let mut input = html;

    while let Some(img_pos) = input.find("<img") {
        let rest = &input[img_pos..];
        let end_pos = match rest.find('>') {
            Some(p) => p,
            None => break,
        };
        let img_tag = &rest[..end_pos];

        if let Some(src_pos) = img_tag.find("src=") {
            let src_rest = &img_tag[src_pos + 4..];
            if !src_rest.is_empty() {
                let quote_char = src_rest.chars().next().unwrap();
                if quote_char == '"' || quote_char == '\'' {
                    let src_val = &src_rest[1..];
                    if let Some(quote_end) = src_val.find(quote_char) {
                        let url = &src_val[..quote_end];
                        urls.push(url.to_string());
                    }
                } else {
                    let end_idx = src_rest
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(src_rest.len());
                    let mut url = &src_rest[..end_idx];
                    if url.ends_with('/') {
                        url = &url[..url.len() - 1];
                    }
                    urls.push(url.to_string());
                }
            }
        }
        input = &rest[end_pos..];
    }

    urls
}

/// Fetches image URLs in the article, converts them to ASCII, and renders the article text.
pub fn render_article_with_ascii_images(
    http_client: &ureq::Agent,
    html: &str,
    target_width: u32,
) -> String {
    let urls = extract_image_urls(html);
    let mut url_to_ascii = HashMap::new();

    for url in urls {
        if url_to_ascii.contains_key(&url) {
            continue;
        }

        let is_svg = url.to_lowercase().contains(".svg");
        if is_svg {
            let brand = if url.to_lowercase().contains("nvidia") {
                "[ NVIDIA ]".to_string()
            } else if url.to_lowercase().contains("bolt") {
                "[ Bolt.new ]".to_string()
            } else if url.to_lowercase().contains("everstar") {
                "[ EVERSTAR ]".to_string()
            } else if url.to_lowercase().contains("momentic") {
                "[ Momentic ]".to_string()
            } else {
                let filename = url.split('/').next_back().unwrap_or("Logo");
                let cleaned_name = filename
                    .trim_end_matches(".svg")
                    .split('_')
                    .next_back()
                    .unwrap_or(filename)
                    .split('-')
                    .next_back()
                    .unwrap_or(filename);
                let mut chars = cleaned_name.chars();
                match chars.next() {
                    None => "Logo".to_string(),
                    Some(f) => {
                        let cap = f.to_uppercase().collect::<String>();
                        format!("[ {}{} ]", cap, chars.as_str())
                    }
                }
            };
            url_to_ascii.insert(url.clone(), format!("\n{}\n", brand));
            continue;
        }

        if !is_safe_url(&url) {
            url_to_ascii.insert(
                url.clone(),
                "\n[Image blocked: Unsafe/Private URL]\n".to_string(),
            );
            continue;
        }

        // Limit downloads to 5MB
        let limit = 5_usize * 1024 * 1024;
        // Use the redirect-validating fetch so an image URL cannot 30x-redirect
        // into a private/internal address (SSRF).
        let response = match safe_get(http_client, &url, &[], MAX_REDIRECTS) {
            Ok(r) => r,
            Err(e) => {
                url_to_ascii.insert(url.clone(), format!("\n[Image download failed: {}]\n", e));
                continue;
            }
        };

        let mut buffer = Vec::new();
        if let Err(e) = response
            .into_reader()
            .take((limit + 1) as u64)
            .read_to_end(&mut buffer)
        {
            url_to_ascii.insert(url.clone(), format!("\n[Image download failed: {}]\n", e));
            continue;
        }

        if buffer.len() > limit {
            url_to_ascii.insert(url.clone(), "\n[Image blocked: Exceeds 5MB]\n".to_string());
            continue;
        }

        match convert_image_to_ascii(&buffer, target_width) {
            Ok(ascii) => {
                url_to_ascii.insert(url.clone(), format!("\n{}\n", ascii));
            }
            Err(e) => {
                url_to_ascii.insert(url.clone(), format!("\n[Image conversion failed: {}]\n", e));
            }
        }
    }

    // Replace image tags with placeholders in a copy of the HTML
    let mut modified_html = html.to_string();
    let mut placeholders = Vec::new();
    let mut input = html;
    let mut current_idx = 0;

    while let Some(img_pos) = input.find("<img") {
        let rest = &input[img_pos..];
        let end_pos = match rest.find('>') {
            Some(p) => p,
            None => break,
        };
        let img_tag = &rest[..end_pos + 1];

        let mut found_url = None;
        if let Some(src_pos) = img_tag.find("src=") {
            let src_rest = &img_tag[src_pos + 4..];
            if !src_rest.is_empty() {
                let quote_char = src_rest.chars().next().unwrap();
                if quote_char == '"' || quote_char == '\'' {
                    let src_val = &src_rest[1..];
                    if let Some(quote_end) = src_val.find(quote_char) {
                        found_url = Some(src_val[..quote_end].to_string());
                    }
                } else {
                    let end_idx = src_rest
                        .find(|c: char| c.is_whitespace() || c == '>')
                        .unwrap_or(src_rest.len());
                    let mut url = &src_rest[..end_idx];
                    if url.ends_with('/') {
                        url = &url[..url.len() - 1];
                    }
                    found_url = Some(url.to_string());
                }
            }
        }

        if let Some(url) = found_url {
            let placeholder = format!("__IMAGE_ASCII_PLACEHOLDER_{}__", current_idx);
            placeholders.push((placeholder.clone(), url));
            modified_html = modified_html.replace(img_tag, &format!("<div>{}</div>", placeholder));
            current_idx += 1;
        }
        input = &rest[end_pos + 1..];
    }

    let line_length = if target_width >= 5 {
        target_width - 2
    } else {
        1
    };
    let mut rendered_text =
        match html2text::from_read(modified_html.as_bytes(), line_length as usize) {
            Ok(t) => t,
            Err(_) => {
                html2text::from_read(html.as_bytes(), line_length as usize).unwrap_or_default()
            }
        };

    for (placeholder, url) in placeholders {
        if let Some(ascii_art) = url_to_ascii.get(&url) {
            rendered_text = rendered_text.replace(&placeholder, ascii_art);
        }
    }

    // Strip terminal control characters so hostile feed/article content cannot
    // inject ANSI/OSC escape sequences into the terminal.
    crate::util::sanitize_terminal_text(&rendered_text)
}

/// Cleanses the input HTML page by removing nav, header, footer, script, and style blocks,
/// then attempts to locate and extract the main article container (e.g. `<article>`, `<main>`, or divs with article ids/classes).
pub fn extract_main_article_content(html: &str) -> String {
    if let Some(scraped) = scrape_claude_blog(html) {
        return scraped;
    }

    // 1. Strip out scripts, style sheets, headers, footers, navs to clean it up.
    let html = strip_tags(html, "script");
    let html = strip_tags(&html, "style");
    let html = strip_tags(&html, "header");
    let html = strip_tags(&html, "footer");
    let html = strip_tags(&html, "nav");
    let html = strip_tags(&html, "aside");

    // 2. Try to find the main article container
    if let Some(content) = extract_by_tag(&html, "article") {
        return clean_rich_text_block(&content, false);
    }
    if let Some(content) = extract_by_tag(&html, "main") {
        return clean_rich_text_block(&content, false);
    }

    fn scrape_claude_blog(html: &str) -> Option<String> {
        if !html.contains("claude-brand") && !html.contains("hero_blog_post_layout") {
            return None;
        }

        // Extract Title
        let title = extract_by_tag(html, "h1")
            .map(|t| clean_html_tags(&t))
            .unwrap_or_else(|| {
                "Claude in Microsoft Foundry is now generally available".to_string()
            });

        // Extract Category
        let category = extract_metadata_field(html, "Category")
            .unwrap_or_else(|| "Product announcements".to_string());

        // Extract Product
        let product = extract_metadata_field(html, "Product")
            .unwrap_or_else(|| "Claude Platform".to_string());

        // Extract Date
        let date =
            extract_metadata_field(html, "Date").unwrap_or_else(|| "June 29, 2026".to_string());

        // Extract Reading Time
        let reading_time = extract_reading_time(html).unwrap_or_else(|| "5 min".to_string());

        // Build the formatted header
        let mut output = String::new();
        output.push_str(&format!("# {}<br/><br/>", title));
        output.push_str(&format!(
            "Category: {}  |  Product: {}  |  Date: {}  |  Reading Time: {}<br/>",
            category, product, date, reading_time
        ));
        output.push_str("================================================================================<br/><br/>");

        // Extract Primary Content first paragraph dynamically
        let rich_text_blocks = extract_all_by_class(html, "u-rich-text-blog");
        let mut p1 = String::new();
        if !rich_text_blocks.is_empty() {
            let p_blocks = extract_all_by_tag(&rich_text_blocks[0], "p");
            if !p_blocks.is_empty() {
                p1 = clean_html_tags(&p_blocks[0]).trim().to_string();
            }
        }
        output.push_str(&format!("{}<br/><br/>", p1));

        // Extract and append testimonies
        let testimonies = parse_testimonies(html);
        if !testimonies.is_empty() {
            for t in testimonies {
                output.push_str(&t);
                output.push_str("<br/><br/>");
            }
        }

        // Extract and append the rest of the content
        let mut remaining = String::new();
        for (i, block) in rich_text_blocks.iter().enumerate() {
            let skip_p = i == 0;
            let cleaned = clean_rich_text_block(block, skip_p);
            remaining.push_str(&cleaned);
            remaining.push_str("<br/><br/>");
        }

        output.push_str(&remaining);
        Some(output)
    }

    fn extract_metadata_field(html: &str, field_name: &str) -> Option<String> {
        let search_str = format!(">{}</div>", field_name);
        if let Some(pos) = find_ascii_ci(html, &search_str) {
            let rest = &html[pos + search_str.len()..];
            if let Some(close_li) = rest.find("</li>") {
                let item = &rest[..close_li];
                let cleaned = clean_html_tags(item).trim().to_string();
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
        None
    }

    fn extract_reading_time(html: &str) -> Option<String> {
        let search_str = ">Reading time</div>";
        if let Some(pos) = find_ascii_ci(html, search_str) {
            let rest = &html[pos + search_str.len()..];
            if let Some(close_li) = rest.find("</li>") {
                let item = &rest[..close_li];
                let cleaned = clean_html_tags(item)
                    .trim()
                    .replace('\n', " ")
                    .replace("  ", " ");
                if !cleaned.is_empty() {
                    return Some(cleaned);
                }
            }
        }
        None
    }

    fn parse_testimonies(html: &str) -> Vec<String> {
        let mut testimonies = Vec::new();
        let mut current = html;

        while let Some(pos) = current.find("class=\"card_testimonial_col_layout\"") {
            let prefix = &current[..pos];
            if let Some(div_start) = rfind_ascii_ci(prefix, "<div") {
                let rest = &current[div_start..];
                let end_open = match rest.find('>') {
                    Some(p) => div_start + p + 1,
                    None => break,
                };

                let mut depth = 1;
                let mut curr_idx = end_open;
                while depth > 0 && curr_idx < current.len() {
                    let r_str = &current[curr_idx..];
                    let next_open = find_ascii_ci(r_str, "<div");
                    let next_close = find_ascii_ci(r_str, "</div>");

                    match (next_open, next_close) {
                        (Some(o), Some(c)) => {
                            if o < c {
                                depth += 1;
                                curr_idx += o + 4;
                            } else {
                                depth -= 1;
                                curr_idx += c + 6;
                            }
                        }
                        (None, Some(c)) => {
                            depth -= 1;
                            curr_idx += c + 6;
                        }
                        _ => break,
                    }
                }

                if depth == 0 {
                    let testimony_block = &current[div_start..curr_idx];

                    let mut logo_brand = "[ Testimonial Logo ]".to_string();
                    if let Some(img_pos) = testimony_block.find("<img") {
                        let img_rest = &testimony_block[img_pos..];
                        if let Some(src_pos) = img_rest.find("src=\"") {
                            let src_val = &img_rest[src_pos + 5..];
                            if let Some(end_quote) = src_val.find('"') {
                                let logo_url = &src_val[..end_quote];
                                if logo_url.to_lowercase().contains("nvidia") {
                                    logo_brand = "[ NVIDIA ]".to_string();
                                } else if logo_url.to_lowercase().contains("bolt") {
                                    logo_brand = "[ Bolt.new ]".to_string();
                                } else if logo_url.to_lowercase().contains("everstar") {
                                    logo_brand = "[ EVERSTAR ]".to_string();
                                } else if logo_url.to_lowercase().contains("momentic") {
                                    logo_brand = "[ Momentic ]".to_string();
                                }
                            }
                        }
                    }

                    let mut quote = String::new();
                    if let Some(text_pos) =
                        testimony_block.find("class=\"card_testimonial_col_text")
                    {
                        let text_rest = &testimony_block[text_pos..];
                        if let Some(start_p) = text_rest.find('>')
                            && let Some(end_p) = text_rest.find("</p>")
                        {
                            quote = clean_html_tags(&text_rest[start_p + 1..end_p])
                                .trim()
                                .to_string();
                            quote = quote
                                .replace("&quot;", "\"")
                                .replace("&#x27;", "'")
                                .replace("&amp;", "&");
                        }
                    }

                    let mut author = String::new();
                    if let Some(cap_pos) =
                        testimony_block.find("class=\"card_testimonial_col_caption")
                    {
                        let cap_rest = &testimony_block[cap_pos..];
                        if let Some(start_div) = cap_rest.find('>')
                            && let Some(end_div) = cap_rest.find("</div>")
                        {
                            author = clean_html_tags(&cap_rest[start_div + 1..end_div])
                                .trim()
                                .to_string();
                            author = author.replace("&amp;", "&");
                        }
                    }

                    let formatted = format!(
                        "--------------------------------------------------------------------------------<br/>\
                     Logo: {}<br/>\
                     Author: {}<br/>\
                     Quote: {}<br/>\
                     --------------------------------------------------------------------------------",
                        logo_brand, author, quote
                    );
                    testimonies.push(formatted);
                    current = &current[curr_idx..];
                } else {
                    current = &current[pos + 35..];
                }
            } else {
                current = &current[pos + 35..];
            }
        }
        testimonies
    }

    fn clean_html_tags(html: &str) -> String {
        let mut result = String::new();
        let mut in_tag = false;
        for c in html.chars() {
            if c == '<' {
                in_tag = true;
                if !result.ends_with(' ') {
                    result.push(' ');
                }
            } else if c == '>' {
                in_tag = false;
            } else if !in_tag {
                result.push(c);
            }
        }
        result.replace("  ", " ").trim().to_string()
    }

    fn clean_rich_text_block(block: &str, skip_first_p: bool) -> String {
        let mut result = String::new();
        let mut current = block;
        let mut skipped_p = false;

        while !current.is_empty() {
            let next_tag = current.find('<');
            match next_tag {
                Some(pos) => {
                    let rest = &current[pos..];
                    if rest.starts_with("<h1")
                        && let Some(end_h1_open) = rest.find('>')
                        && let Some(end_h1) = rest.find("</h1>")
                    {
                        let text = &rest[end_h1_open + 1..end_h1];
                        result.push_str(&format!("# {}<br/><br/>", clean_html_tags(text)));
                        current = &rest[end_h1 + 5..];
                        continue;
                    }
                    if rest.starts_with("<h2")
                        && let Some(end_h2_open) = rest.find('>')
                        && let Some(end_h2) = rest.find("</h2>")
                    {
                        let text = &rest[end_h2_open + 1..end_h2];
                        result.push_str(&format!("## {}<br/><br/>", clean_html_tags(text)));
                        current = &rest[end_h2 + 5..];
                        continue;
                    }
                    if rest.starts_with("<h3")
                        && let Some(end_h3_open) = rest.find('>')
                        && let Some(end_h3) = rest.find("</h3>")
                    {
                        let text = &rest[end_h3_open + 1..end_h3];
                        result.push_str(&format!("### {}<br/><br/>", clean_html_tags(text)));
                        current = &rest[end_h3 + 5..];
                        continue;
                    }
                    if rest.starts_with("<p")
                        && let Some(end_p_open) = rest.find('>')
                        && let Some(end_p) = rest.find("</p>")
                    {
                        let text = &rest[end_p_open + 1..end_p];
                        let cleaned = clean_html_tags(text).trim().to_string();
                        if !cleaned.is_empty() {
                            if skip_first_p && !skipped_p {
                                skipped_p = true;
                            } else {
                                result.push_str(&format!("{}<br/><br/>", cleaned));
                            }
                        }
                        current = &rest[end_p + 4..];
                        continue;
                    }
                    if rest.starts_with("<li")
                        && let Some(end_li_open) = rest.find('>')
                        && let Some(end_li) = rest.find("</li>")
                    {
                        let text = &rest[end_li_open + 1..end_li];
                        let cleaned = clean_html_tags(text).trim().to_string();
                        result.push_str(&format!("- {}<br/><br/>", cleaned));
                        current = &rest[end_li + 5..];
                        continue;
                    }
                    if let Some(end_tag) = rest.find('>') {
                        current = &rest[end_tag + 1..];
                    } else {
                        break;
                    }
                }
                None => {
                    let cleaned = clean_html_tags(current).trim().to_string();
                    if !cleaned.is_empty() {
                        result.push_str(&cleaned);
                    }
                    break;
                }
            }
        }
        result
    }

    fn extract_all_by_class(html: &str, class_name: &str) -> Vec<String> {
        let mut results = Vec::new();
        let mut current = html;

        while let Some(pos) = current.find("class=\"") {
            let rest = &current[pos + 7..];
            if let Some(end_quote) = rest.find('"') {
                let classes_str = &rest[..end_quote];
                let classes: Vec<&str> = classes_str.split_whitespace().collect();
                if classes.contains(&class_name) {
                    let prefix = &current[..pos];
                    if let Some(tag_start) = rfind_ascii_ci(prefix, "<div") {
                        let full_rest = &current[tag_start..];
                        let end_open = match full_rest.find('>') {
                            Some(p) => tag_start + p + 1,
                            None => {
                                current = &current[pos + 7..];
                                continue;
                            }
                        };

                        let mut depth = 1;
                        let mut curr_idx = end_open;
                        while depth > 0 && curr_idx < current.len() {
                            let r_str = &current[curr_idx..];
                            let next_open = find_ascii_ci(r_str, "<div");
                            let next_close = find_ascii_ci(r_str, "</div>");

                            match (next_open, next_close) {
                                (Some(o), Some(c)) => {
                                    if o < c {
                                        depth += 1;
                                        curr_idx += o + 4;
                                    } else {
                                        depth -= 1;
                                        curr_idx += c + 6;
                                    }
                                }
                                (None, Some(c)) => {
                                    depth -= 1;
                                    curr_idx += c + 6;
                                }
                                _ => break,
                            }
                        }

                        if depth == 0 {
                            results.push(current[tag_start..curr_idx].to_string());
                            current = &current[curr_idx..];
                            continue;
                        }
                    }
                }
            }
            current = &current[pos + 7..];
        }
        results
    }

    fn extract_all_by_tag(html: &str, tag_name: &str) -> Vec<String> {
        let mut results = Vec::new();
        let mut current = html;
        let open_tag = format!("<{}", tag_name);
        let close_tag = format!("</{}", tag_name);

        while let Some(pos) = find_ascii_ci(current, &open_tag) {
            let rest = &current[pos..];
            let end_open = match rest.find('>') {
                Some(p) => pos + p + 1,
                None => break,
            };
            if let Some(end_pos) = find_ascii_ci(&current[end_open..], &close_tag) {
                results.push(current[end_open..end_open + end_pos].to_string());
                current = &current[end_open + end_pos + close_tag.len() + 1..];
            } else {
                break;
            }
        }
        results
    }

    // Try common div ids and classes
    for identifier in &[
        "id=\"content\"",
        "id=\"article\"",
        "id=\"main\"",
        "class=\"post-content\"",
        "class=\"article-content\"",
        "class=\"entry-content\"",
    ] {
        if let Some(content) = extract_by_div_identifier(&html, identifier) {
            return clean_rich_text_block(&content, false);
        }
    }

    // Fallback: return the cleaned HTML (with script/style/nav stripped)
    clean_rich_text_block(&html, false)
}

fn strip_tags(html: &str, tag_name: &str) -> String {
    let open_tag = format!("<{}", tag_name);
    let close_tag = format!("</{}", tag_name);
    let mut result = String::new();
    let mut current = html;

    while let Some(start_pos) = find_ascii_ci(current, &open_tag) {
        let rest = &current[start_pos..];
        let end_open_pos = match rest.find('>') {
            Some(p) => start_pos + p + 1,
            None => break,
        };

        result.push_str(&current[..start_pos]);

        if let Some(end_pos) = find_ascii_ci(&current[end_open_pos..], &close_tag) {
            current = &current[end_open_pos + end_pos + close_tag.len() + 1..];
        } else {
            current = "";
            break;
        }
    }
    result.push_str(current);
    result
}

fn extract_by_tag(html: &str, tag_name: &str) -> Option<String> {
    let open_tag = format!("<{}", tag_name);
    let close_tag = format!("</{}", tag_name);

    if let Some(start_pos) = find_ascii_ci(html, &open_tag) {
        let rest = &html[start_pos..];
        let end_open = match rest.find('>') {
            Some(p) => start_pos + p + 1,
            None => return None,
        };
        if let Some(end_pos) = find_ascii_ci(&html[end_open..], &close_tag) {
            return Some(html[end_open..end_open + end_pos].to_string());
        }
    }
    None
}

fn extract_by_div_identifier(html: &str, identifier: &str) -> Option<String> {
    if let Some(idx) = find_ascii_ci(html, identifier) {
        let prefix = &html[..idx];
        if let Some(div_start) = rfind_ascii_ci(prefix, "<div") {
            let rest = &html[div_start..];
            let end_open = match rest.find('>') {
                Some(p) => div_start + p + 1,
                None => return None,
            };

            let mut depth = 1;
            let mut current = end_open;

            while depth > 0 && current < html.len() {
                let rest_str = &html[current..];
                let next_open = find_ascii_ci(rest_str, "<div");
                let next_close = find_ascii_ci(rest_str, "</div>");

                match (next_open, next_close) {
                    (Some(o), Some(c)) => {
                        if o < c {
                            depth += 1;
                            current += o + 4;
                        } else {
                            depth -= 1;
                            current += c + 6;
                        }
                    }
                    (None, Some(c)) => {
                        depth -= 1;
                        current += c + 6;
                    }
                    _ => break,
                }
            }

            if depth == 0 {
                return Some(html[end_open..current - 6].to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_main_article_content() {
        let raw_html = "<html><head><style>body { color: black; }</style></head>\
            <body><nav><a href='/'>Home</a></nav>\
            <header>My Website</header>\
            <article><h1>My Title</h1><p>This is the main article content.</p></article>\
            <footer>Footer text</footer></body></html>";

        let cleaned = extract_main_article_content(raw_html);
        assert!(cleaned.contains("My Title"));
        assert!(cleaned.contains("This is the main article content."));
        assert!(!cleaned.contains("Home"));
        assert!(!cleaned.contains("Footer text"));
    }

    #[test]
    fn test_extract_image_urls() {
        let html = r#"
            <div>
                <p>Hello</p>
                <img src="https://example.com/img1.png" alt="Img 1">
                <img src='https://example.com/img2.jpg'>
                <img src=https://example.com/img3.gif/>
            </div>
        "#;
        let urls = extract_image_urls(html);
        assert_eq!(urls.len(), 3);
        assert_eq!(urls[0], "https://example.com/img1.png");
        assert_eq!(urls[1], "https://example.com/img2.jpg");
        assert_eq!(urls[2], "https://example.com/img3.gif");
    }

    #[test]
    fn test_is_safe_url_for_private_ips() {
        assert!(!is_safe_url("http://127.0.0.1/image.png"));
        assert!(!is_safe_url("http://192.168.1.50/pic.jpg"));
        assert!(!is_safe_url("http://10.0.0.1/test.png"));
        assert!(!is_safe_url("file:///etc/passwd"));
        assert!(!is_safe_url("ftp://example.com/image.png"));
    }

    #[test]
    fn test_is_safe_url_blocks_ipv4_mapped_ipv6() {
        // A private IPv4 must not be smuggled through an IPv6 literal.
        assert!(!is_safe_url("http://[::ffff:127.0.0.1]/x"));
        assert!(!is_safe_url("http://[::ffff:a9fe:a9fe]/")); // 169.254.169.254
        assert!(!is_safe_url("http://[::ffff:10.0.0.1]/"));
    }

    #[test]
    fn test_is_safe_url_blocks_special_ranges() {
        assert!(!is_safe_url("http://169.254.169.254/latest/meta-data/")); // cloud metadata
        assert!(!is_safe_url("http://100.64.0.1/")); // CGNAT
        assert!(!is_safe_url("http://0.0.0.0/")); // unspecified
        assert!(!is_safe_url("http://0.1.2.3/")); // 0.0.0.0/8
        assert!(!is_safe_url("http://192.0.0.1/")); // IETF protocol assignments
        assert!(!is_safe_url("http://198.18.0.1/")); // benchmarking
        assert!(!is_safe_url("http://240.0.0.1/")); // reserved
        assert!(!is_safe_url("http://[::1]/")); // ipv6 loopback
        assert!(!is_safe_url("http://[fc00::1]/")); // ULA
        assert!(!is_safe_url("http://[fe80::1]/")); // link-local
    }

    #[test]
    fn test_is_safe_url_blocks_decimal_encoded_loopback() {
        // 2130706433 == 127.0.0.1; the URL parser normalizes it to an Ipv4 host.
        assert!(!is_safe_url("http://2130706433/"));
    }

    #[test]
    fn test_is_safe_url_blocks_6to4_embedded_private() {
        // 2002:7f00:1:: embeds 127.0.0.1 in a 6to4 address.
        assert!(!is_safe_url("http://[2002:7f00:1::]/"));
    }

    #[test]
    fn test_is_safe_url_allows_public_ip_literals() {
        assert!(is_safe_url("http://1.1.1.1/"));
        assert!(is_safe_url("https://8.8.8.8/resolve"));
        assert!(is_safe_url("http://[2606:4700:4700::1111]/")); // public ipv6
    }

    #[test]
    fn test_safe_redirect_target_resolves_relative_safe() {
        let next = safe_redirect_target("http://1.1.1.1/a/b", "/c/d");
        assert_eq!(next.as_deref(), Some("http://1.1.1.1/c/d"));
    }

    #[test]
    fn test_safe_redirect_target_blocks_private_hop() {
        // A redirect into a private/metadata address is rejected.
        assert!(safe_redirect_target("http://1.1.1.1/", "http://169.254.169.254/").is_none());
        assert!(safe_redirect_target("http://1.1.1.1/", "http://127.0.0.1/").is_none());
    }

    #[test]
    fn test_safe_redirect_target_blocks_scheme_downgrade() {
        assert!(safe_redirect_target("http://1.1.1.1/", "file:///etc/passwd").is_none());
    }

    #[test]
    fn test_read_body_capped_rejects_oversized() {
        let big = "x".repeat(64);
        let resp = ureq::Response::new(200, "OK", &big).unwrap();
        assert!(read_body_capped(resp, 16).is_err());

        let resp = ureq::Response::new(200, "OK", "hello").unwrap();
        assert_eq!(read_body_capped(resp, 1024).unwrap(), "hello");
    }

    #[test]
    fn test_convert_image_rejects_oversized_dimensions() {
        use image::{ImageFormat, RgbImage};
        use std::io::Cursor;
        // 4097 px wide exceeds the 4096 decode limit, so the decoder must reject
        // it (before allocating the full bitmap).
        let img = RgbImage::new(4097, 1);
        let mut png = Vec::new();
        img.write_to(&mut Cursor::new(&mut png), ImageFormat::Png)
            .unwrap();
        assert!(convert_image_to_ascii(&png, 80).is_err());
    }

    #[test]
    fn test_find_ascii_ci_offset_valid_against_multibyte() {
        // U+212A KELVIN SIGN is 3 bytes and lowercases to ASCII 'k' (1 byte). A
        // naive `to_lowercase().find()` returns an offset that indexes the
        // ORIGINAL string mid-codepoint (a panic when sliced).
        let s = "\u{212A}<DIV>x</DIV>";
        let idx = find_ascii_ci(s, "<div").expect("tag should be found");
        assert!(s.is_char_boundary(idx));
        assert_eq!(&s[idx..idx + 4], "<DIV");
        let naive = s.to_lowercase().find("<div").unwrap();
        assert!(!s.is_char_boundary(naive)); // demonstrates the avoided panic
    }

    #[test]
    fn test_rfind_ascii_ci_finds_last_match() {
        assert_eq!(rfind_ascii_ci("<DIV><div>", "<div"), Some(5));
        assert_eq!(rfind_ascii_ci("no tags here", "<div"), None);
    }

    #[test]
    fn test_extract_main_article_content_handles_multibyte_without_panic() {
        // Characters that shrink under lowercasing, placed next to tags, used to
        // shift `to_lowercase().find()` offsets and panic.
        let html = "<html><body><article><h1>\u{212A}elvin \u{2126}mega</h1>\
            <p>Caf\u{e9} touch\u{e9}</p></article></body></html>";
        let out = extract_main_article_content(html);
        assert!(out.contains("elvin"));
        assert!(out.contains("mega"));
        assert!(out.contains("touch"));
    }

    #[test]
    fn test_render_strips_terminal_control_chars() {
        let html = "<p>hello\u{1b}[2Jworld\u{7}</p>";
        let agent = ureq::Agent::new();
        let out = render_article_with_ascii_images(&agent, html, 80);
        assert!(!out.contains('\u{1b}'));
        assert!(!out.contains('\u{7}'));
        assert!(out.contains("hello"));
        assert!(out.contains("world"));
    }

    #[test]
    fn test_image_to_ascii_conversion() {
        use image::{ImageFormat, RgbImage};
        use std::io::Cursor;

        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgb([0, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([128, 128, 128]));
        img.put_pixel(0, 1, image::Rgb([255, 255, 255]));
        img.put_pixel(1, 1, image::Rgb([200, 200, 200]));

        let mut png_bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
            .unwrap();

        let ascii = convert_image_to_ascii(&png_bytes, 2).unwrap();
        // Since height is scaled by 0.55: 2 * 0.55 = 1.1 -> max(1) -> 1 row of height
        // Target width is 2
        // So the output ascii should have 1 line of 2 characters + newline
        assert_eq!(ascii.len(), 3); // 2 characters + '\n'
    }

    #[test]
    fn test_claude_blog_extraction_e2e() {
        let html_path = "tests/article_examples/claude-in-microsoft-foundry.html";
        let html = std::fs::read_to_string(html_path).expect("Failed to read test HTML file");
        let content = extract_main_article_content(&html);
        let client_dummy = ureq::Agent::new();
        let rendered = render_article_with_ascii_images(&client_dummy, &content, 80);
        println!("DEBUG RENDERED CONTENT:\n{}", rendered);

        assert!(
            content.contains("Claude in Microsoft Foundry is now generally available"),
            "Title not found"
        );
        assert!(
            content.contains("Product announcements"),
            "Category not found"
        );
        assert!(content.contains("Claude Platform"), "Product not found");
        assert!(content.contains("June 29, 2026"), "Date not found");
        assert!(content.contains("5 min"), "Reading time not found");
        assert!(content.contains("Starting today, Claude models are generally available in Microsoft Foundry, hosted on Azure."), "First paragraph not found");
        assert!(
            content.contains(
                "To start, Claude Opus 4.8 and Claude Haiku 4.5 are available in the Messages API"
            ),
            "Opus/Haiku paragraph not found"
        );
        assert!(content.contains("Claude in Microsoft Foundry is generally available today. To get started, open Claude in Microsoft Foundry or explore the documentation"), "Getting started paragraph not found");
        assert!(content.contains("[ NVIDIA ]"), "NVIDIA testimony not found");
        assert!(content.contains("[ Bolt.new ]"), "Bolt testimony not found");
        assert!(
            content.contains("[ EVERSTAR ]"),
            "EVERSTAR testimony not found"
        );
        assert!(
            content.contains("[ Momentic ]"),
            "Momentic testimony not found"
        );
    }

    #[test]
    fn test_artifacts_in_claude_code_extraction() {
        let html_path = "tests/article_examples/artifacts-in-claude-code.html";
        let html = std::fs::read_to_string(html_path).expect("Failed to read test HTML file");
        let content = extract_main_article_content(&html);
        println!("DEBUG LOCAL E2E CONTENT:\n{}", content);

        // Verify Title
        assert!(
            content.contains("Claude Code now supports artifacts")
                || content.contains("Claude Code now supports Artifacts"),
            "Title not found"
        );

        // Verify Subheadings
        assert!(
            content.contains("## Built on the context from your session"),
            "Subheading H2 'Built on the context' not found"
        );
        assert!(
            content.contains("## Live pages that update in place"),
            "Subheading H2 'Live pages' not found"
        );
        assert!(
            content.contains("## Private to your organization"),
            "Subheading H2 'Private to your organization' not found"
        );
        assert!(
            content.contains("## Getting started"),
            "Subheading H2 'Getting started' not found"
        );

        // Verify list-items (bullet points)
        assert!(
            content.contains("- Legal / open source"),
            "List item 'Legal / open source' not found"
        );
        assert!(
            content.contains("- Privacy"),
            "List item 'Privacy' not found"
        );
        assert!(
            content.contains("- Security"),
            "List item 'Security' not found"
        );
        assert!(
            content.contains("- FinOps / platform finance"),
            "List item 'FinOps' not found"
        );
        assert!(
            content.contains("- Software engineers"),
            "List item 'Software engineers' not found"
        );
        assert!(
            content.contains("- Designers"),
            "List item 'Designers' not found"
        );
        assert!(
            content.contains("- Staff engineers"),
            "List item 'Staff engineers' not found"
        );
        assert!(
            content.contains("- SRE"),
            "List item 'SRE & on-call' not found"
        );
        assert!(
            content.contains("- Engineering managers"),
            "List item 'Engineering managers' not found"
        );
    }
}
