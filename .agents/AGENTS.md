# Project Rules & Customizations

This file outlines the rules and guidelines for this workspace.

## Workflow Rules

- **Focused Commits**: Always make small, logically focused commits rather than mega-commits. This makes revision history easy to parse.
- **Testing**: Always run `cargo test` after code changes to ensure everything compiles and passes all unit tests.
- **Pause for Verification**: For any user-facing changes (UI adjustments, new keybindings, visual states), **pause after implementation and your own testing** so that the user can verify the results. Do not rush to the next feature without explicit user feedback.

## Implementation Details

- **Direct Rust HTTP Layer**: Any network requests (such as LLM summarization) must be implemented directly in Rust using the existing dependency tree (`ureq` + `serde_json`), avoiding external runtimes like Python or complex wrapper libraries.
- **SSRF & Resource Safety**:
  - Image downloads for ASCII art conversion must check URLs to block private/internal IP ranges (e.g. localhost, private subnets).
  - Impose a strict maximum download size limit (e.g., 5MB) on image downloads to protect against decompression/resource bombs.
- **No Hardcoded Dynamic Content**: Never hardcode content that is clearly dynamic (such as user-fed content or downloaded webpage contents). All scraped, fetched, or input content must be processed and extracted dynamically.
