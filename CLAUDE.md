# Developer Guide & Safety Rules

## Build, Test & Run Commands
- Build: `cargo build`
- Run: `cargo run`
- Test: `cargo test`
- Release build: `cargo build --release`

## Quality, Safety & Vulnerability Guidelines

- **Security & Safety First**: Never compromise on security. Carefully analyze any implementation for potential vulnerabilities (such as SSRF, input sanitization issues, resource leaks, or buffer/integer overflows) before submission.
- **SSRF Mitigation**:
  - Image downloads for ASCII art conversion must validate URLs to block private/internal IP ranges (e.g., localhost, private subnets like `127.0.0.0/8`, `10.0.0.0/8`, `172.16.0.0/12`, `192.168.0.0/16`).
  - Enforce a strict maximum download size limit (e.g., 5MB) on image downloads to protect against decompression or resource bombs.
- **No Hardcoded Dynamic Content**: Never hardcode content that is clearly dynamic (such as user-fed content or downloaded webpage contents). All scraped, fetched, or input content must be processed and extracted dynamically.
- **Regression Testing**: Always write automated regression tests for any bug findings. Place tests inside the `tests/` directory as integration tests where possible, or as unit tests inside the module.
