# Rustenati

High-performance Rust CLI for dumping genealogical records from Portale Antenati (antenati.cultura.gov.it).

## Project Overview

- **Language:** Rust (edition 2024)
- **Async runtime:** Tokio
- **CLI framework:** Clap (derive)
- **Target:** 132M+ images across 1.6M IIIF manifests of Italian civil records

## Build & Run

```bash
cargo build --release
cargo run -- --help
cargo test
```

## Architecture

Single binary crate. Key modules:
- `client/` - HTTP client, IIIF parsing, WAF handling, rate limiting
- `download/` - parallel download engine with SQLite state tracking
- `ocr/` - pluggable OCR backends (trait-based)
- `models/` - data structures for manifests, search results, ARK identifiers
- `cli/` - clap command definitions

## Portale Antenati API

The portal uses Apache Solr for search and IIIF v2/v3 for image serving.

**Critical:** All requests must include `Referer: https://antenati.cultura.gov.it/` header.

Key domains:
- `antenati.cultura.gov.it` - search, portal
- `dam-antenati.cultura.gov.it` - IIIF v2, DAM, manifests
- `iiif-antenati.cultura.gov.it` - IIIF v3

See `docs/api-reference.md` for full endpoint documentation.

## Conventions

- Use `thiserror` for typed errors in library code, `anyhow` at CLI boundary
- All HTTP calls go through `AntenatiClient` (never raw reqwest)
- OCR backends implement the `OcrBackend` trait
- State tracking via SQLite (rusqlite with bundled feature)
- Progress bars via indicatif
- Tests: unit tests inline, integration tests in `tests/`, fixtures in `tests/fixtures/`

## Dependencies Policy

- Prefer well-maintained crates with minimal transitive dependencies
- Feature-gate optional OCR backends
- Use `bundled` SQLite to avoid system dependency
