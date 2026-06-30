//! This module provides a way to asynchronously refresh feeds, using threads

use crate::ReadOptions;
use crate::app::App;
use crate::modes::Mode;
use anyhow::Result;

/// Upper bound on a fetched article page we will read into memory.
const MAX_ARTICLE_BYTES: usize = 10 * 1024 * 1024;

pub enum Action {
    Break,
    RefreshFeed(crate::rss::FeedId),
    RefreshFeeds(Vec<crate::rss::FeedId>),
    SubscribeToFeed(String),
    ClearFlash,
    RenderAsciiArticle(crate::rss::EntryId, u32),
    SummarizeArticle(crate::rss::EntryId),
    FetchModels,
    /// Load persisted chat history for an entry into the app state.
    OpenChat(crate::rss::EntryId),
    /// Run one chat turn (RAG + agentic loop) for an entry given the user message.
    SendChatMessage(crate::rss::EntryId, String),
}

/// A loop to process `io::Action` messages.
pub fn io_loop(
    app: App,
    io_tx: std::sync::mpsc::Sender<Action>,
    io_rx: std::sync::mpsc::Receiver<Action>,
    options: &ReadOptions,
) -> Result<()> {
    let manager = r2d2_sqlite::SqliteConnectionManager::file(&options.database_path);
    let connection_pool = r2d2::Pool::new(manager)?;

    while let Ok(event) = io_rx.recv() {
        match event {
            Action::Break => break,
            Action::RefreshFeed(feed_id) => {
                let now = std::time::Instant::now();

                app.set_flash("Refreshing feed...".to_string());
                app.force_redraw()?;

                refresh_feeds(&app, &connection_pool, &[feed_id], |_app, fetch_result| {
                    if let Err(e) = fetch_result {
                        app.push_error_flash(e)
                    }
                })?;

                app.update_current_feed_and_entries()?;
                let elapsed = now.elapsed();
                app.set_flash(format!("Refreshed feed in {elapsed:?}"));
                app.force_redraw()?;
                clear_flash_after(io_tx.clone(), options.flash_display_duration_seconds);
            }
            Action::RefreshFeeds(feed_ids) => {
                let now = std::time::Instant::now();

                app.set_flash("Refreshing all feeds...".to_string());
                app.force_redraw()?;

                let all_feeds_len = feed_ids.len();
                let mut successfully_refreshed_len = 0usize;

                refresh_feeds(&app, &connection_pool, &feed_ids, |app, fetch_result| {
                    match fetch_result {
                        Ok(_) => successfully_refreshed_len += 1,
                        Err(e) => app.push_error_flash(e),
                    }
                })?;

                {
                    app.update_current_feed_and_entries()?;

                    let elapsed = now.elapsed();
                    app.set_flash(format!(
                        "Refreshed {successfully_refreshed_len}/{all_feeds_len} feeds in {elapsed:?}"
                    ));
                    app.force_redraw()?;
                }

                clear_flash_after(io_tx.clone(), options.flash_display_duration_seconds);
            }
            Action::SubscribeToFeed(feed_subscription_input) => {
                let now = std::time::Instant::now();

                app.set_flash("Subscribing to feed...".to_string());
                app.force_redraw()?;

                let mut conn = connection_pool.get()?;
                let r = crate::rss::subscribe_to_feed(
                    &app.http_client(),
                    &mut conn,
                    &feed_subscription_input,
                );

                if let Err(e) = r {
                    app.push_error_flash(e);
                    continue;
                }

                match crate::rss::get_feeds(&conn) {
                    Ok(feeds) => {
                        {
                            app.reset_feed_subscription_input();
                            app.set_feeds(feeds);
                            app.select_feeds();
                            app.update_current_feed_and_entries()?;

                            let elapsed = now.elapsed();
                            app.set_flash(format!("Subscribed in {elapsed:?}"));
                            app.set_mode(Mode::Normal);
                            app.force_redraw()?;
                        }

                        clear_flash_after(io_tx.clone(), options.flash_display_duration_seconds);
                    }
                    Err(e) => {
                        app.push_error_flash(e);
                    }
                }
            }
            Action::ClearFlash => {
                app.clear_flash();
            }
            Action::RenderAsciiArticle(entry_id, target_width) => {
                let conn = connection_pool.get()?;
                match crate::rss::get_entry_content(&conn, entry_id) {
                    Ok(entry_content) => {
                        let empty_string = String::from("No content or description tag provided.");
                        let mut html = entry_content
                            .content
                            .as_ref()
                            .or(entry_content.description.as_ref())
                            .unwrap_or(&empty_string)
                            .clone();

                        if let Ok(entry_meta) = crate::rss::get_entry_meta(&conn, entry_id)
                            && let Some(link) = &entry_meta.link
                            && crate::ascii::is_safe_url(link)
                        {
                            app.set_flash("Fetching full article content...".to_string());
                            let _ = app.force_redraw();

                            let client = app.http_client();
                            if let Ok(resp) = crate::ascii::safe_get(
                                &client,
                                link,
                                &[],
                                crate::ascii::MAX_REDIRECTS,
                            ) && let Ok(resp_body) =
                                crate::ascii::read_body_capped(resp, MAX_ARTICLE_BYTES)
                            {
                                let cleaned_html =
                                    crate::ascii::extract_main_article_content(&resp_body);
                                if !cleaned_html.trim().is_empty() {
                                    html = cleaned_html;
                                }
                            }
                        }

                        let http_client = app.http_client();
                        let rendered_text = crate::ascii::render_article_with_ascii_images(
                            &http_client,
                            &html,
                            target_width,
                        );

                        if let Ok(entry_meta) = crate::rss::get_entry_meta(&conn, entry_id) {
                            app.set_entry_ascii_content(rendered_text, entry_meta);
                            app.clear_flash();
                            app.force_redraw()?;
                        }
                    }
                    Err(e) => {
                        app.push_error_flash(e);
                        app.force_redraw()?;
                    }
                }
            }
            Action::SummarizeArticle(entry_id) => {
                app.set_flash("Summarizing article using LLM...".to_string());
                app.force_redraw()?;

                let conn = connection_pool.get()?;
                match fetch_article_text(&app, &conn, entry_id) {
                    Ok(text) => match crate::summarize::summarize_article(&text) {
                        Ok(summary) => {
                            // The IO path builds `text` at a fixed 80-col wrap,
                            // but select_and_show_current_entry looks up the cache
                            // using current_entry_text (pane-width wrap).  Write a
                            // second cache entry under that key so the next redraw
                            // doesn't blank the box.
                            let entry_text = app.current_entry_text();
                            if !entry_text.is_empty() {
                                let s = app.settings();
                                let alt_payload = crate::llm::build_prompt_payload(
                                    &entry_text,
                                    s.max_words_per_prompt,
                                );
                                let _ = crate::cache::insert_cached_summary(
                                    &alt_payload,
                                    &s.model_name,
                                    crate::summarize::SUMMARIZE_SYSTEM_PROMPT,
                                    &summary,
                                );
                            }
                            app.set_current_summary(Some(summary));
                            app.clear_flash();
                            app.force_redraw()?;
                        }
                        Err(e) => {
                            app.set_flash(format!("Summarization failed: {}", e));
                            app.push_error_flash(e);
                            app.force_redraw()?;
                        }
                    },
                    Err(e) => {
                        app.set_flash(format!("Summarization failed: {}", e));
                        app.push_error_flash(e);
                        app.force_redraw()?;
                    }
                }
            }
            Action::FetchModels => {
                app.set_flash("Fetching available models...".to_string());
                app.force_redraw()?;

                let settings = app.settings();
                match crate::llm::fetch_available_models(&settings.base_url, &settings.api_key_env)
                {
                    Ok(models) => {
                        app.set_available_models(models);
                        app.set_flash("Models fetched successfully!".to_string());
                        app.force_redraw()?;
                    }
                    Err(e) => {
                        // Redact any API key the provider error may carry (e.g. the
                        // Gemini `?key=` URL ureq embeds) before showing it.
                        let redacted = crate::llm::redact_secrets(&e.to_string());
                        app.set_flash(format!("Fetch models failed: {}", redacted));
                        app.push_error_flash(anyhow::anyhow!(redacted));
                        app.force_redraw()?;
                    }
                }
            }
            Action::OpenChat(entry_id) => {
                // Ensure a session exists and load any persisted history into view.
                let settings = app.settings();
                let _ = crate::cache::get_or_create_chat_session(entry_id, &settings.model_name);
                match crate::cache::load_chat_messages(entry_id) {
                    Ok(stored) => app.set_chat_messages(stored_to_display(&stored)),
                    Err(e) => {
                        app.set_chat_messages(Vec::new());
                        app.push_error_flash(e);
                    }
                }
                app.force_redraw()?;
            }
            Action::SendChatMessage(entry_id, message) => {
                let settings = app.settings();
                let conn = connection_pool.get()?;
                let result = fetch_article_text(&app, &conn, entry_id).and_then(|article_text| {
                    let progress_app = app.clone();
                    crate::chat::run_chat_turn(
                        &settings,
                        entry_id,
                        &article_text,
                        &message,
                        |status: &str| {
                            progress_app.set_flash(status.to_string());
                            let _ = progress_app.force_redraw();
                        },
                    )
                });
                match result {
                    Ok(answer) => {
                        app.push_chat_message(crate::app::ChatTurn {
                            role: "assistant".to_string(),
                            content: answer,
                        });
                        app.clear_flash();
                    }
                    Err(e) => {
                        let redacted = crate::llm::redact_secrets(&e.to_string());
                        app.set_flash(format!("Chat failed: {}", redacted));
                        app.push_error_flash(anyhow::anyhow!(redacted));
                    }
                }
                // Always clear the in-flight flag so the input is usable again.
                app.set_chat_in_flight(false);
                app.force_redraw()?;
            }
        }
    }

    Ok(())
}

/// Fetches and cleans an entry's article text the same way summarization does:
/// the stored content/description, replaced by the live page's extracted main
/// content when the link is SSRF-safe and fetchable, rendered to plain text at a
/// fixed 80-column wrap.
fn fetch_article_text(
    app: &App,
    conn: &rusqlite::Connection,
    entry_id: crate::rss::EntryId,
) -> Result<String> {
    let entry_content = crate::rss::get_entry_content(conn, entry_id)?;
    let empty_string = String::from("No content or description tag provided.");
    let mut html = entry_content
        .content
        .as_ref()
        .or(entry_content.description.as_ref())
        .unwrap_or(&empty_string)
        .clone();

    if let Ok(entry_meta) = crate::rss::get_entry_meta(conn, entry_id)
        && let Some(link) = &entry_meta.link
        && crate::ascii::is_safe_url(link)
    {
        let client = app.http_client();
        if let Ok(resp) = crate::ascii::safe_get(&client, link, &[], crate::ascii::MAX_REDIRECTS)
            && let Ok(resp_body) = crate::ascii::read_body_capped(resp, MAX_ARTICLE_BYTES)
        {
            let cleaned_html = crate::ascii::extract_main_article_content(&resp_body);
            if !cleaned_html.trim().is_empty() {
                html = cleaned_html;
            }
        }
    }

    Ok(match html2text::from_read(html.as_bytes(), 80) {
        Ok(t) => t,
        Err(_) => html,
    })
}

/// Maps persisted chat messages to display turns: user and non-empty assistant
/// text turns pass through; an assistant tool-call turn becomes a "searched the
/// web" marker; raw tool-result turns are hidden.
fn stored_to_display(stored: &[crate::cache::StoredChatMessage]) -> Vec<crate::app::ChatTurn> {
    let mut turns = Vec::new();
    for m in stored {
        match m.role.as_str() {
            "user" => turns.push(crate::app::ChatTurn {
                role: "user".to_string(),
                content: m.content.clone(),
            }),
            "assistant" if !m.content.is_empty() => turns.push(crate::app::ChatTurn {
                role: "assistant".to_string(),
                content: m.content.clone(),
            }),
            "assistant" => turns.push(crate::app::ChatTurn {
                role: "tool".to_string(),
                content: "🔍 searched the web".to_string(),
            }),
            _ => {} // hide raw tool-result turns
        }
    }
    turns
}

/// Refreshes the feeds of the given `feed_ids` by splitting them into
/// chunks based on the number of available CPUs.
/// Each chunk is then passed to its own thread,
/// where each feed_id in the chunk has its feed refreshed synchronously on that thread.
fn refresh_feeds<F>(
    app: &App,
    connection_pool: &r2d2::Pool<r2d2_sqlite::SqliteConnectionManager>,
    feed_ids: &[crate::rss::FeedId],
    mut refresh_result_handler: F,
) -> Result<()>
where
    F: FnMut(&App, anyhow::Result<()>),
{
    let chunks = chunkify_for_threads(feed_ids, num_cpus::get() * 2);

    let join_handles: Vec<_> = chunks
        .map(|chunk| {
            let pool_get_result = connection_pool.get();
            let http_client = app.http_client();
            let chunk = chunk.to_owned();

            std::thread::spawn(move || -> Result<Vec<Result<(), anyhow::Error>>> {
                let mut conn = pool_get_result?;

                let results = chunk
                    .into_iter()
                    .map(|feed_id| crate::rss::refresh_feed(&http_client, &mut conn, feed_id))
                    .collect();

                Ok::<Vec<Result<(), anyhow::Error>>, anyhow::Error>(results)
            })
        })
        .collect();

    for join_handle in join_handles {
        let chunk_results = join_handle
            .join()
            .expect("unable to join worker thread to io thread");
        for chunk_result in chunk_results? {
            refresh_result_handler(app, chunk_result)
        }
    }

    Ok(())
}

/// split items into chunks,
/// with the idea being that each chunk will be run on its own thread
fn chunkify_for_threads<T>(
    items: &[T],
    minimum_number_of_threads: usize,
) -> impl Iterator<Item = &[T]> {
    // example: 25 items / 16 threads = chunk size of 1
    // example: 100 items / 16 threads = chunk size of 6
    // example: 10 items / 16 threads = chunk size of 0 (handled later)
    //
    // due to usize floor division, it's possible chunk_size would be 0,
    // so ensure it is at least 1
    let chunk_size = (items.len() / minimum_number_of_threads).max(1);

    // now we have (len / chunk_size) chunks,
    // example:
    // 25 items / chunks size of 1 = 25 chunks
    // 100 items / chunk size of 6 = 16 chunks
    items.chunks(chunk_size)
}

/// clear the flash after a given duration
fn clear_flash_after(tx: std::sync::mpsc::Sender<Action>, duration: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(duration);
        tx.send(Action::ClearFlash)
            .expect("Unable to send IOCommand::ClearFlash");
    });
}
