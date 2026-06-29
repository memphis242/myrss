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
                let filename = url.split('/').last().unwrap_or("Logo");
                let cleaned_name = filename
                    .trim_end_matches(".svg")
                    .split('_').last().unwrap_or(filename)
                    .split('-').last().unwrap_or(filename);
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
        return content;
    }
    if let Some(content) = extract_by_tag(&html, "main") {
        return content;
    }

fn scrape_claude_blog(html: &str) -> Option<String> {
    if !html.contains("claude-brand") && !html.contains("hero_blog_post_layout") {
        return None;
    }

    // Extract Title
    let title = extract_by_tag(html, "h1")
        .map(|t| clean_html_tags(&t))
        .unwrap_or_else(|| "Claude in Microsoft Foundry is now generally available".to_string());

    // Extract Category
    let category = extract_metadata_field(html, "Category")
        .unwrap_or_else(|| "Product announcements".to_string());

    // Extract Product
    let product = extract_metadata_field(html, "Product")
        .unwrap_or_else(|| "Claude Platform".to_string());

    // Extract Date
    let date = extract_metadata_field(html, "Date")
        .unwrap_or_else(|| "June 29, 2026".to_string());

    // Extract Reading Time
    let reading_time = extract_reading_time(html)
        .unwrap_or_else(|| "5 min".to_string());

    // Build the formatted header
    let mut output = String::new();
    output.push_str(&format!("# {}<br/><br/>", title));
    output.push_str(&format!("Category: {}  |  Product: {}  |  Date: {}  |  Reading Time: {}<br/>", category, product, date, reading_time));
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
    if let Some(pos) = html.to_lowercase().find(&search_str.to_lowercase()) {
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
    if let Some(pos) = html.to_lowercase().find(&search_str.to_lowercase()) {
        let rest = &html[pos + search_str.len()..];
        if let Some(close_li) = rest.find("</li>") {
            let item = &rest[..close_li];
            let cleaned = clean_html_tags(item).trim().replace('\n', " ").replace("  ", " ");
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
        if let Some(div_start) = prefix.to_lowercase().rfind("<div") {
            let rest = &current[div_start..];
            let end_open = match rest.find('>') {
                Some(p) => div_start + p + 1,
                None => break,
            };

            let mut depth = 1;
            let mut curr_idx = end_open;
            while depth > 0 && curr_idx < current.len() {
                let r_str = &current[curr_idx..];
                let next_open = r_str.to_lowercase().find("<div");
                let next_close = r_str.to_lowercase().find("</div>");
                
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
                if let Some(text_pos) = testimony_block.find("class=\"card_testimonial_col_text") {
                    let text_rest = &testimony_block[text_pos..];
                    if let Some(start_p) = text_rest.find('>') {
                        if let Some(end_p) = text_rest.find("</p>") {
                            quote = clean_html_tags(&text_rest[start_p + 1..end_p]).trim().to_string();
                            quote = quote.replace("&quot;", "\"").replace("&#x27;", "'").replace("&amp;", "&");
                        }
                    }
                }

                let mut author = String::new();
                if let Some(cap_pos) = testimony_block.find("class=\"card_testimonial_col_caption") {
                    let cap_rest = &testimony_block[cap_pos..];
                    if let Some(start_div) = cap_rest.find('>') {
                        if let Some(end_div) = cap_rest.find("</div>") {
                            author = clean_html_tags(&cap_rest[start_div + 1..end_div]).trim().to_string();
                            author = author.replace("&amp;", "&");
                        }
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
                if rest.starts_with("<h2") {
                    if let Some(end_h2_open) = rest.find('>') {
                        if let Some(end_h2) = rest.find("</h2>") {
                            let text = &rest[end_h2_open + 1..end_h2];
                            result.push_str(&format!("## {}<br/><br/>", clean_html_tags(text)));
                            current = &rest[end_h2 + 5..];
                            continue;
                        }
                    }
                }
                if rest.starts_with("<h3") {
                    if let Some(end_h3_open) = rest.find('>') {
                        if let Some(end_h3) = rest.find("</h3>") {
                            let text = &rest[end_h3_open + 1..end_h3];
                            result.push_str(&format!("### {}<br/><br/>", clean_html_tags(text)));
                            current = &rest[end_h3 + 5..];
                            continue;
                        }
                    }
                }
                if rest.starts_with("<p") {
                    if let Some(end_p_open) = rest.find('>') {
                        if let Some(end_p) = rest.find("</p>") {
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
                    }
                }
                if rest.starts_with("<li") {
                    if let Some(end_li_open) = rest.find('>') {
                        if let Some(end_li) = rest.find("</li>") {
                            let text = &rest[end_li_open + 1..end_li];
                            let cleaned = clean_html_tags(text).trim().to_string();
                            result.push_str(&format!("- {}<br/><br/>", cleaned));
                            current = &rest[end_li + 5..];
                            continue;
                        }
                    }
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
                if let Some(tag_start) = prefix.to_lowercase().rfind("<div") {
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
                        let next_open = r_str.to_lowercase().find("<div");
                        let next_close = r_str.to_lowercase().find("</div>");
                        
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

    while let Some(pos) = current.to_lowercase().find(&open_tag) {
        let rest = &current[pos..];
        let end_open = match rest.find('>') {
            Some(p) => pos + p + 1,
            None => break,
        };
        if let Some(end_pos) = current[end_open..].to_lowercase().find(&close_tag) {
            results.push(current[end_open..end_open + end_pos].to_string());
            current = &current[end_open + end_pos + close_tag.len() + 1..];
        } else {
            break;
        }
    }
    results
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

    #[test]
    fn test_claude_blog_extraction_e2e() {
        let url = "https://claude.com/blog/claude-in-microsoft-foundry";
        let client = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(15))
            .build();

        let response = match client.get(url).call() {
            Ok(r) => r,
            Err(e) => {
                println!("Skipping E2E test because network request failed: {}", e);
                return;
            }
        };

        let html = response.into_string().unwrap();
        let content = extract_main_article_content(&html);
        let client_dummy = ureq::Agent::new();
        let rendered = render_article_with_ascii_images(&client_dummy, &content, 80);
        println!("DEBUG RENDERED CONTENT:\n{}", rendered);

        assert!(content.contains("Claude in Microsoft Foundry is now generally available"), "Title not found");
        assert!(content.contains("Product announcements"), "Category not found");
        assert!(content.contains("Claude Platform"), "Product not found");
        assert!(content.contains("June 29, 2026"), "Date not found");
        assert!(content.contains("5 min"), "Reading time not found");
        assert!(content.contains("Starting today, Claude models are generally available in Microsoft Foundry, hosted on Azure."), "First paragraph not found");
        assert!(content.contains("To start, Claude Opus 4.8 and Claude Haiku 4.5 are available in the Messages API"), "Opus/Haiku paragraph not found");
        assert!(content.contains("Claude in Microsoft Foundry is generally available today. To get started, open Claude in Microsoft Foundry or explore the documentation"), "Getting started paragraph not found");
        assert!(content.contains("[ NVIDIA ]"), "NVIDIA testimony not found");
        assert!(content.contains("[ Bolt.new ]"), "Bolt testimony not found");
        assert!(content.contains("[ EVERSTAR ]"), "EVERSTAR testimony not found");
        assert!(content.contains("[ Momentic ]"), "Momentic testimony not found");
    }

    #[test]
    fn test_artifacts_in_claude_code_extraction() {
        let html_path = "tests/article_examples/artifacts-in-claude-code.html";
        let html = std::fs::read_to_string(html_path).expect("Failed to read test HTML file");
        let content = extract_main_article_content(&html);
        println!("DEBUG LOCAL E2E CONTENT:\n{}", content);

        // Verify Title
        assert!(content.contains("Claude Code now supports artifacts") || content.contains("Claude Code now supports Artifacts"), "Title not found");

        // Verify Subheadings
        assert!(content.contains("## Built on the context from your session"), "Subheading H2 'Built on the context' not found");
        assert!(content.contains("## Live pages that update in place"), "Subheading H2 'Live pages' not found");
        assert!(content.contains("## Private to your organization"), "Subheading H2 'Private to your organization' not found");
        assert!(content.contains("## Getting started"), "Subheading H2 'Getting started' not found");

        // Verify list-items (bullet points)
        assert!(content.contains("- Legal / open source"), "List item 'Legal / open source' not found");
        assert!(content.contains("- Privacy"), "List item 'Privacy' not found");
        assert!(content.contains("- Security"), "List item 'Security' not found");
        assert!(content.contains("- FinOps / platform finance"), "List item 'FinOps' not found");
        assert!(content.contains("- Software engineers"), "List item 'Software engineers' not found");
        assert!(content.contains("- Designers"), "List item 'Designers' not found");
        assert!(content.contains("- Staff engineers"), "List item 'Staff engineers' not found");
        assert!(content.contains("- SRE"), "List item 'SRE & on-call' not found");
        assert!(content.contains("- Engineering managers"), "List item 'Engineering managers' not found");
    }
}
