use std::collections::HashMap;
use std::io::Read;
use std::net::{IpAddr, ToSocketAddrs};
use url::Url;
use image::GenericImageView;

/// Checks if the given URL is safe to download from (prevents SSRF and loopback requests).
pub fn is_safe_url(url_str: &str) -> bool {
    let url = match Url::parse(url_str) {
        Ok(u) => u,
        Err(_) => return false,
    };

    // Only allow http and https protocols
    if url.scheme() != "http" && url.scheme() != "https" {
        return false;
    }

    let host = match url.host_str() {
        Some(h) => h,
        None => return false,
    };

    // Resolve domain/host to IP addresses
    let socket_addr_str = format!("{}:80", host);
    let addrs = match socket_addr_str.to_socket_addrs() {
        Ok(iter) => iter,
        Err(_) => return false,
    };

    for addr in addrs {
        let ip = addr.ip();
        if is_private_ip(ip) {
            return false;
        }
    }

    true
}

/// Checks if an IP Address is loopback, private, or local.
fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ipv4) => {
            ipv4.is_loopback()
                || ipv4.is_private()
                || ipv4.is_link_local()
                || ipv4.is_multicast()
                || ipv4.is_broadcast()
                || ipv4.is_unspecified()
        }
        IpAddr::V6(ipv6) => {
            ipv6.is_loopback()
                || ipv6.is_unspecified()
                || (ipv6.segments()[0] & 0xfe00) == 0xfc00 // Unique Local Address (ULA)
                || (ipv6.segments()[0] & 0xffc0) == 0xfe80 // Link-Local
                || ipv6.is_multicast()
        }
    }
}

/// Converts a buffer of image bytes to ASCII art, enforcing size checks and aspect ratio correction.
pub fn convert_image_to_ascii(bytes: &[u8], target_width: u32) -> anyhow::Result<String> {
    // Decode image from memory
    let img = image::load_from_memory(bytes)?;

    // Decompression bomb protection: Reject if image is excessively large
    let (width, height) = img.dimensions();
    if width > 4096 || height > 4096 {
        anyhow::bail!("Image dimensions too large: {}x{}", width, height);
    }

    // Terminal characters are about twice as tall as they are wide.
    // Adjust target height by 0.5 to preserve the original aspect ratio.
    let aspect_ratio = height as f32 / width as f32;
    let target_height = ((target_width as f32 * aspect_ratio) * 0.55).max(1.0) as u32;

    // Fast scale the image
    let resized = img.resize_exact(target_width, target_height, image::imageops::FilterType::Nearest);
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

        if !is_safe_url(&url) {
            url_to_ascii.insert(url.clone(), "\n[Image blocked: Unsafe/Private URL]\n".to_string());
            continue;
        }

        // Limit downloads to 5MB
        let limit = 5_usize * 1024 * 1024;
        let response = match http_client
            .get(&url)
            .set("User-Agent", "myrss/0.5.0")
            .call()
        {
            Ok(r) => r,
            Err(e) => {
                url_to_ascii.insert(url.clone(), format!("\n[Image download failed: {}]\n", e));
                continue;
            }
        };

        let mut buffer = Vec::new();
        if let Err(e) = response.into_reader().take((limit + 1) as u64).read_to_end(&mut buffer) {
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

    let line_length = if target_width >= 5 { target_width - 2 } else { 1 };
    let mut rendered_text = match html2text::from_read(modified_html.as_bytes(), line_length as usize) {
        Ok(t) => t,
        Err(_) => html2text::from_read(html.as_bytes(), line_length as usize).unwrap_or_default(),
    };

    for (placeholder, url) in placeholders {
        if let Some(ascii_art) = url_to_ascii.get(&url) {
            rendered_text = rendered_text.replace(&placeholder, ascii_art);
        }
    }

    rendered_text
}

/// Cleanses the input HTML page by removing nav, header, footer, script, and style blocks,
/// then attempts to locate and extract the main article container (e.g. `<article>`, `<main>`, or divs with article ids/classes).
pub fn extract_main_article_content(html: &str) -> String {
    // 1. Strip out scripts, style sheets, headers, footers, navs to clean it up.
    let html = strip_tags(html, "script");
    let html = strip_tags(&html, "style");
    let html = strip_tags(&html, "header");
    let html = strip_tags(&html, "footer");
    let html = strip_tags(&html, "nav");
    let html = strip_tags(&html, "aside");

    // 2. Try to find the main article container
    if let Some(content) = extract_by_tag(&html, "article") {
        return content;
    }
    if let Some(content) = extract_by_tag(&html, "main") {
        return content;
    }

    // Try common div ids and classes
    for identifier in &["id=\"content\"", "id=\"article\"", "id=\"main\"", "class=\"post-content\"", "class=\"article-content\"", "class=\"entry-content\""] {
        if let Some(content) = extract_by_div_identifier(&html, identifier) {
            return content;
        }
    }

    // Fallback: return the cleaned HTML (with script/style/nav stripped)
    html
}

fn strip_tags(html: &str, tag_name: &str) -> String {
    let open_tag = format!("<{}", tag_name);
    let close_tag = format!("</{}", tag_name);
    let mut result = String::new();
    let mut current = html;

    while let Some(start_pos) = current.to_lowercase().find(&open_tag) {
        let rest = &current[start_pos..];
        let end_open_pos = match rest.find('>') {
            Some(p) => start_pos + p + 1,
            None => break,
        };

        result.push_str(&current[..start_pos]);

        if let Some(end_pos) = current[end_open_pos..].to_lowercase().find(&close_tag) {
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
    
    if let Some(start_pos) = html.to_lowercase().find(&open_tag) {
        let rest = &html[start_pos..];
        let end_open = match rest.find('>') {
            Some(p) => start_pos + p + 1,
            None => return None,
        };
        if let Some(end_pos) = html[end_open..].to_lowercase().find(&close_tag) {
            return Some(html[end_open..end_open + end_pos].to_string());
        }
    }
    None
}

fn extract_by_div_identifier(html: &str, identifier: &str) -> Option<String> {
    if let Some(idx) = html.to_lowercase().find(identifier) {
        let prefix = &html[..idx];
        if let Some(div_start) = prefix.to_lowercase().rfind("<div") {
            let rest = &html[div_start..];
            let end_open = match rest.find('>') {
                Some(p) => div_start + p + 1,
                None => return None,
            };
            
            let mut depth = 1;
            let mut current = end_open;
            
            while depth > 0 && current < html.len() {
                let rest_str = &html[current..];
                let next_open = rest_str.to_lowercase().find("<div");
                let next_close = rest_str.to_lowercase().find("</div>");
                
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
    fn test_image_to_ascii_conversion() {
        use image::{ImageFormat, RgbImage};
        use std::io::Cursor;

        let mut img = RgbImage::new(2, 2);
        img.put_pixel(0, 0, image::Rgb([0, 0, 0]));
        img.put_pixel(1, 0, image::Rgb([128, 128, 128]));
        img.put_pixel(0, 1, image::Rgb([255, 255, 255]));
        img.put_pixel(1, 1, image::Rgb([200, 200, 200]));

        let mut png_bytes = Vec::new();
        img.write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png).unwrap();

        let ascii = convert_image_to_ascii(&png_bytes, 2).unwrap();
        // Since height is scaled by 0.55: 2 * 0.55 = 1.1 -> max(1) -> 1 row of height
        // Target width is 2
        // So the output ascii should have 1 line of 2 characters + newline
        assert_eq!(ascii.len(), 3); // 2 characters + '\n'
    }
}
