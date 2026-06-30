# Security Defenses

This document records the security review of MyRSS's **network-facing** and
**configuration** attack surfaces: for each surface it states the threat, whether
the original code addressed it, and the defense-in-depth measure that was applied.
It is meant to be kept up to date as the code evolves.

MyRSS fetches arbitrary remote feeds, web pages, and images, and optionally sends
article text to a third-party LLM. **All remote input is treated as hostile.** The
guiding principle (see `CLAUDE.md`) is *correctness and safety over speed*, with
*defense-in-depth*: keep existing guardrails and add layers rather than replacing
them.

## Threat model at a glance

| # | Surface | Primary threat | Status before | Defense applied |
|---|---------|----------------|---------------|-----------------|
| 1 | Outbound URL validation (`ascii::is_safe_url`) | SSRF to private/loopback/metadata IPs | Partial (basic ranges, IPv4-mapped IPv6 bypass, fail-open on empty DNS) | Comprehensive special-use denylist incl. embedded-IPv4-in-IPv6; fail-closed |
| 2 | HTTP redirects | Redirect-based SSRF bypass (validate URL, then 302 → `169.254.169.254`) | **Unaddressed** (auto-followed, only initial URL checked) | `safe_get` re-validates every hop; shared agent set to `redirects(0)` |
| 3 | Response body size | Memory exhaustion from unbounded body | Partial (implicit 10 MB only for `into_string`) | Explicit, tested byte caps for feeds & article HTML |
| 4 | Image decoding (`ascii::convert_image_to_ascii`) | Decompression bomb expands before the dimension check | Partial (post-decode check only) | `image::Limits` enforce dimensions/allocation *during* decode |
| 5 | Untrusted HTML parsing (`ascii.rs`) | Panic / mis-parse via `to_lowercase()` offset shift on multibyte input (DoS) | **Unaddressed** | ASCII-case-insensitive search on original byte offsets |
| 6 | LLM error logging (`llm.rs`) | API key leak — `ureq` errors embed the Gemini `?key=` URL into the on-disk log/UI | **Unaddressed** | `redact_secrets` scrubs keys/bearer tokens before logging/display |
| 7 | LLM prompt construction (`llm.rs`) | Prompt injection by closing the `</article_text>` wrapper | Partial (wrapper + system instruction) | Delimiter neutralization in a shared payload builder |
| 8 | LLM retry backoff (`llm.rs`) | `Duration` overflow panic / unbounded sleep with large `max_retries` | **Unaddressed** | Exponential backoff capped |
| 9 | Configuration (`settings.rs`) | Hand-edited `config.json` → infinite timeout / runaway cost | **Unaddressed** | Settings clamped to sane bounds on load |
| 10 | Terminal rendering | ANSI/OSC escape injection from feed titles & article bodies | **Unaddressed** | Control-character sanitization of feed-derived text |

The sections below give detail and point at the regression tests that guard each
fix.

---

## 1. SSRF — outbound URL validation

**Where:** `ascii::is_safe_url`, `ascii::is_disallowed_ip`.

**Threat.** The app fetches image URLs and "full article" links taken straight
from hostile feed content. Without validation these can target internal services
(`http://127.0.0.1:…`), the cloud metadata endpoint (`http://169.254.169.254/…`),
or other private ranges — classic Server-Side Request Forgery.

**Before.** `is_safe_url` blocked http(s)-only and a handful of ranges, but:
- It formatted `host:80` and called `to_socket_addrs`; on a host that resolved to
  **zero** addresses the loop body never ran and the function returned `true`
  (fail-open).
- IPv6 checks missed **IPv4-mapped** addresses (`::ffff:127.0.0.1`) and other
  embedded-IPv4 forms, so a private IPv4 could be smuggled through an IPv6 literal.
- It missed several special-use IPv4 ranges (CGNAT `100.64/10`, `0.0.0.0/8`,
  `192.0.0.0/24`, benchmarking `198.18/15`, reserved `240/4`).

**Applied.**
- Use `url.host()` so IP-literal hosts (including the decimal/hex/octal encodings
  the URL parser normalizes, e.g. `http://2130706433/`) are checked directly.
- A broad special-use **denylist** (`is_disallowed_ipv4`/`is_disallowed_ipv6`),
  since the stdlib has no stable `is_global()`. IPv6 addresses that embed IPv4
  (mapped, compatible, 6to4, Teredo) are unwrapped and re-checked under the IPv4
  rules.
- **Fail closed**: a domain that resolves to no address is rejected.

**Residual risk.** DNS rebinding (TOCTOU) between this pre-flight check and the
socket connect is not fully closed for the connect itself — `ureq` re-resolves.
This is mitigated for the *redirect* vector by #2, and the denylist still blocks
the common metadata/loopback targets, but a determined rebind attack remains a
known limitation (documented here rather than silently ignored).

**Tests.** `ascii::tests::test_is_safe_url_*` (private/loopback, IPv4-mapped IPv6,
CGNAT, 0/8, reserved, decimal-encoded IP, fail-closed).

## 2. Redirect-based SSRF

**Where:** `ascii::safe_get`, `ascii::safe_redirect_target`; `App` HTTP agent.

**Threat.** Even with #1, a server can pass the up-front check and then issue a
`302 Location: http://169.254.169.254/…`. `ureq` followed redirects automatically,
so only the *initial* URL was ever validated.

**Applied.** The shared HTTP agent is built with `.redirects(0)`. All untrusted
fetches go through `safe_get`, which follows only real redirect codes
(301/302/303/307/308 — **not** 304, so conditional feed GETs still work),
re-validates each hop's absolute target with `is_safe_url`, and caps the hop count
(`MAX_REDIRECTS`). The hop-resolution logic is factored into the pure function
`safe_redirect_target` for hermetic testing.

**Tests.** `ascii::tests::test_safe_redirect_target_*`.

## 3. Response body size limits

**Where:** `ascii::read_body_capped`; `rss::fetch_feed`; `io.rs` article fetch.

**Threat.** A hostile server can stream an unbounded body to exhaust memory.

**Before.** Feeds relied on `ureq`'s implicit 10 MB `into_string` limit; images
were already capped at 5 MB.

**Applied.** Explicit, intentional byte caps (`read_body_capped`) for feed bodies
and fetched article HTML, independent of `ureq` internals. Images keep their 5 MB
cap.

**Tests.** `ascii::tests::test_read_body_capped_rejects_oversized`.

## 4. Image decompression bombs

**Where:** `ascii::convert_image_to_ascii`.

**Threat.** A small compressed image can decode to an enormous bitmap.

**Before.** Dimensions were checked **after** `image::load_from_memory` fully
decoded the image — the bomb had already expanded.

**Applied.** Decode through `image::ImageReader` with `image::Limits`
(`max_image_width`/`max_image_height` = 4096, bounded `max_alloc`), which reject
oversized images *before* allocating the full buffer. The post-decode dimension
check is kept as a second layer.

**Tests.** `ascii::tests::test_convert_image_rejects_oversized_dimensions`.

## 5. Malicious-HTML parsing panic / DoS

**Where:** the linear-scan extractors in `ascii.rs`.

**Threat.** The extractors located tags with `slice.to_lowercase().find("<div")`
and then indexed the **original** slice with that offset. `to_lowercase()` can
change a string's byte length (e.g. `U+212A KELVIN SIGN` → `k`), so on multibyte
input the offset is wrong and can land mid-codepoint — slicing there **panics**
(a remote DoS, since the HTML is fetched from the article link).

**Applied.** `find_ascii_ci`/`rfind_ascii_ci` perform ASCII case-insensitive
search and return offsets valid for the *original* string. Tag/attribute names are
ASCII, so behavior is unchanged for normal input while the panic class is removed.
The scans stay linear (no ReDoS).

**Tests.** `ascii::tests::test_find_ascii_ci_*`,
`ascii::tests::test_extract_main_article_content_handles_multibyte_without_panic`,
plus the existing E2E fixtures stay green.

## 6. API-key leakage into logs/UI

**Where:** `llm::redact_secrets`; `llm::summarize_article`; `io.rs`.

**Threat.** `ureq`'s error `Display` embeds the failing request URL. Gemini carries
its API key as a `?key=…` query parameter, so a failed request wrote the key into
the on-disk `request_log` table and showed it in the UI.

**Applied.** `redact_secrets` scrubs `key=`/`Bearer`/`x-api-key` values before any
error is logged or surfaced.

**Tests.** `llm::tests::test_redact_secrets_scrubs_gemini_key_url`,
`llm::tests::test_redact_secrets_scrubs_bearer_token`.

## 7. LLM prompt injection

**Where:** `llm::build_prompt_payload`, `llm::neutralize_delimiters`.

**Threat.** Article text is wrapped in `<article_text>…</article_text>` and the
system prompt says to ignore instructions inside. A crafted article containing the
literal `</article_text>` could close the wrapper early and have following text
treated as instructions.

**Applied.** A shared `build_prompt_payload` neutralizes any literal wrapper tags
in the content before wrapping. Routing both the live call and the cache lookup
through it keeps cache keys consistent.

**Tests.** `llm::tests::test_neutralize_delimiters`,
`llm::tests::test_build_prompt_payload_*`.

## 8. Retry backoff overflow

**Where:** `llm::execute_with_retry`.

**Threat.** Backoff doubled a `Duration` each attempt; a large user `max_retries`
could overflow (panic) or sleep absurdly long.

**Applied.** The delay is computed by `next_backoff`, which uses `saturating_mul`
and is capped at `MAX_BACKOFF`. (`max_retries` is additionally bounded by #9.)

**Tests.** `llm::tests::test_backoff_is_capped`.

## 9. Configuration hardening

**Where:** `settings::AppSettings::clamp`, `settings::load_settings`.

**Threat.** `config.json` is user-editable; absurd values (`timeout_seconds: 0`
→ infinite timeout, huge `max_retries`/`max_words_per_prompt`) cause hangs or
runaway token cost.

**Applied.** Loaded settings are clamped to sane ranges.

**Tests.** `settings::tests::test_settings_are_clamped`.

## 10. Terminal escape injection

**Where:** `util::sanitize_terminal_text`; applied at feed parse time (`rss.rs`)
and to rendered article bodies.

**Threat.** Feed titles and article text are drawn to a terminal. Embedded ANSI/OSC
escape sequences could manipulate the terminal (rewrite the title bar, move the
cursor, etc.).

**Applied.** C0/C1 control characters (except `\n`/`\t`) are stripped from
feed-derived titles/authors and from rendered article text.

**Residual risk.** Feed rows already stored in the database before this change are
sanitized at render only where the render path was updated; re-subscribing rewrites
them cleanly.

**Tests.** `util::tests::test_sanitize_terminal_text`,
`ascii::tests::test_render_strips_terminal_control_chars`.

---

## Surfaces reviewed and judged already-adequate

- **XML entity expansion / XXE** in feed and OPML parsing: `rss`,
  `atom_syndication`, and `opml` are built on `quick-xml`, which does not resolve
  external entities or expand internal entity definitions by default, so
  billion-laughs / XXE are mitigated by parser choice. No code change; noted for
  future-dependency vigilance.
- **SQL injection:** all queries use `rusqlite` bound parameters; the only dynamic
  SQL is a fixed read-mode predicate chosen from an enum. No change.
- **`#![forbid(unsafe_code)]`** remains set.
