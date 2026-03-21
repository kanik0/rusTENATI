# rusTENATI

High-performance Rust CLI for downloading genealogical records from [Portale Antenati](https://antenati.cultura.gov.it) — the digital archive of Italian State Archives containing **132+ million images** of civil records (births, deaths, marriages) from the 16th to 19th century.

## Features

- **Search** by person name or registry (locality, year, document type) with paginated results
- **Download** IIIF images with parallel downloads, rate limiting, retry with exponential backoff, and resume support
- **Batch download** entire search results in a single command
- **Web interface** — local web server to browse, filter, and view all downloaded documents
- **OCR** handwritten historical documents with 4 pluggable backends (Claude Vision, Transkribus, Azure Document Intelligence, Google Cloud Vision)
- **Tag extraction** — automatically extract surnames, names, dates, locations, roles from OCR results
- **SQLite state tracking** for download resume, session history, and local tag search
- **Graceful shutdown** — Ctrl+C saves progress, resume later with `--resume`
- **WAF handling** — automatic detection and resolution of AWS WAF challenges
- **Interactive dashboard** — real-time TUI showing download progress, stats, and disk usage
- **Thumbnail generation** — batch create JPEG thumbnails from downloaded images
- **Integrity verification** — SHA256 checksum validation with auto-fix for corrupted downloads
- **JSON output** — `--json` flag on every command for scripting and pipelines

## Quick Start

```bash
# Build
cargo build --release

# Search for registries in a locality
rustenati search registry --locality Napoli --year-from 1807 --doc-type Nati

# Search by person name
rustenati search name --surname Rossi --name Mario --locality Napoli

# Inspect a manifest (see pages, metadata)
rustenati info https://antenati.cultura.gov.it/ark:/12657/an_ua18771
rustenati info <MANIFEST_URL> --full    # show all canvases

# Download specific pages
rustenati download <MANIFEST_URL> --pages 1-10 --dry-run   # preview
rustenati download <MANIFEST_URL> --pages 1-10              # download

# Download all images from a manifest
rustenati download <MANIFEST_URL> -j 4 --delay 500

# Resume an interrupted download
rustenati download <MANIFEST_URL> --resume

# Batch download: search + download all matching registries
rustenati download --search --locality Napoli --year-from 1807 --doc-type Nati --max-registries 50

# Browse archives
rustenati browse archives                              # list all ~120 Archivi di Stato
rustenati browse archives --filter lucca               # filter by name

# Search registries by archive
rustenati search registry --archive archivio-di-stato-di-lucca --all
rustenati search registry --archive archivio-di-stato-di-lucca --doc-type Nati --year-from 1807

# Dump an entire archive (all registries)
rustenati download --search --archive archivio-di-stato-di-lucca --max-registries 5000
rustenati download --search --archive archivio-di-stato-di-lucca --doc-type Nati --dry-run

# Filter batch results by locality name (case-insensitive)
rustenati download --search --archive archivio-di-stato-di-massa --doc-type Nati --dry-run --filter massa --all

# NOAH MODE: dump the ENTIRE portal (all archives, all registries)
rustenati download --noah --dry-run                   # preview what would be downloaded
rustenati download --noah --resume -j 8 --rps 5       # aggressive download with resume
rustenati download --noah --max-archives 10           # limit to first 10 archives
rustenati download --noah --doc-type Nati --resume    # only birth records, resumable
```

## Performance Tuning

```bash
# Increase parallel downloads (default: 4)
rustenati download <URL> -j 8

# Set explicit rate limit (requests per second, overrides --delay)
rustenati download <URL> --rps 10

# Increase HTTP connection pool (default: 10 per host)
rustenati download <URL> --connections 20

# Aggressive but polite: 8 workers, 5 req/s, 20 connections
rustenati download --noah --resume -j 8 --rps 5 --connections 20

# Maximum throughput (use responsibly!)
rustenati download --search --archive archivio-di-stato-di-lucca -j 16 --rps 20 --connections 30 --delay 0
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `-j, --jobs` | 4 | Concurrent download tasks |
| `--delay` | 500ms | Per-request delay (politeness) |
| `--rps` | derived from delay | Explicit rate limit (req/s) |
| `--connections` | 10 | HTTP pool idle connections per host |

## Document Types (`--doc-type`)

The portal classifies registries by document type. Use the exact value with `--doc-type`:

**Main categories:**

| Value | Description |
|-------|-------------|
| `Nati` | Birth records |
| `Morti` | Death records |
| `Matrimoni` | Marriage records |
| `Cittadinanze` | Citizenship records |
| `Stato delle Anime` | Census of souls (parish records) |

**With sub-types** (indexes, attachments, combined registers):

| Births | Deaths | Marriages |
|--------|--------|-----------|
| `Nati` | `Morti` | `Matrimoni` |
| `Nati, allegati` | `Morti, allegati` | `Matrimoni, allegati` |
| `Nati, esposti` | `Morti, ospedale` | `Matrimoni, pubblicazioni` |
| `Nati, indice` | `Morti, indice` | `Matrimoni, pubblicazioni allegati` |
| `Nati, indici decennali` | `Morti, indici decennali` | `Matrimoni, pubblicazioni indice` |
| `Nati, indici quinquennali` | `Morti, indici quinquennali` | `Matrimoni, pubblicazioni indici decennali` |
| `Nati, indici ventennali` | `Morti, indici ventennali` | `Matrimoni, indice` |
| `Nati, indici trentennali` | `Morti, indici trentennali` | `Matrimoni, indici decennali` |
| | `Morti, indici cinquantennali` | `Matrimoni, indici quinquennali` |
| | `Morti, indici venticiquennali` | `Matrimoni, indici ventennali` |
| | `Estratti dell'atto di morte` | `Matrimoni, indici trentennali` |
| | | `Matrimoni, ecclesiastici annotazioni` |

**Combined registers** (multiple event types in one volume):

`Nati-Matrimoni-Morti` | `Nati-Morti` | `Nati-Pubblicazioni` | `Nati-Pubblicazioni-Matrimoni-Morti` | `Nati-Pubblicazioni-Morti` | `Pubblicazioni-Matrimoni` | `Pubblicazioni-Matrimoni-Morti` | `Pubblicazioni-Morti` | `Matrimoni-Morti, allegati` | `Nati-Matrimoni-Morti, allegati` | `Nati-Matrimoni-Morti, indice` | `Nati-Matrimoni-Morti, indici decennali` | `Nati-Matrimoni-Morti, indici trentennali` | `Nati-Morti-Cittadinanze, indici decennali` | `Vari, allegati`

> **Tip:** The value is case-sensitive and must match exactly. When in doubt, use `rustenati search registry --locality Roma --all --json | jq '[.results[].doc_type] | unique'` to discover available types for a locality.

## Archives (`--archive`)

121 Archivi di Stato are available. See the [full list](docs/archives.md) with slugs.

Most common examples:

```
archivio-di-stato-di-napoli        archivio-di-stato-di-roma
archivio-di-stato-di-palermo       archivio-di-stato-di-firenze
archivio-di-stato-di-bari          archivio-di-stato-di-torino
archivio-di-stato-di-catania       archivio-di-stato-di-bologna
archivio-di-stato-di-lucca         archivio-di-stato-di-caserta
```

You can also list/filter them dynamically: `rustenati browse archives --filter lucca`

## OCR

rusTENATI supports 4 OCR backends for transcribing handwritten Italian civil records:

| Backend | Best for | Accuracy on historical IT | Requires |
|---------|----------|--------------------------|----------|
| **Claude Vision** | General handwriting, contextual understanding | High | `ANTHROPIC_API_KEY` |
| **Transkribus** | Historical Italian manuscripts (XVI-XIX sec.) | Highest | `TRANSKRIBUS_API_KEY` |
| **Azure Document Intelligence** | Semi-legible handwriting | Good | `AZURE_OCR_API_KEY` + `AZURE_OCR_ENDPOINT` |
| **Google Cloud Vision** | Multilingual documents | Good | `GOOGLE_API_KEY` |

```bash
# OCR a single image
rustenati ocr ./image.jpg --backend claude --doc-type birth

# OCR an entire directory with tag extraction
rustenati ocr ./output/images/ --backend claude --extract-tags -j 3

# Extract tags from existing transcriptions
rustenati tags extract ./output/ocr/ --backend claude --doc-type birth
```

## Tags

After OCR, rusTENATI automatically extracts structured data:

- **Surnames** and **names** of all people mentioned
- **Dates** (birth, death, marriage)
- **Locations** mentioned in the act
- **Event type** (birth, death, marriage, baptism)
- **Roles** (father, mother, witness, civil officer)
- **Professions**

```bash
# Search tags across all downloads
rustenati tags search --surname Rossi --locality Napoli

# List tags for a specific download
rustenati tags list --download-id 42

# Manually add a tag
rustenati tags add 42 --tag-type surname --value "DE LUCA"

# View statistics
rustenati tags stats
```

## Status & Sessions

```bash
# Show overall status (manifests, downloads, tags)
rustenati status

# Show all download sessions
rustenati status --all

# Show a specific session
rustenati status --session 5
```

## Verify

```bash
# Full integrity check (SHA256 verification against DB)
rustenati verify

# Quick check (existence + non-zero size only, skip SHA256)
rustenati verify --quick

# Verify a specific manifest
rustenati verify --manifest <MANIFEST_ID>

# Auto-fix: re-queue corrupted/missing files for re-download
rustenati verify --fix
rustenati verify --fix --quick    # fast fix pass

# JSON output for scripting
rustenati verify --json
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `--quick` | off | Skip SHA256, only check file existence and non-zero size |
| `--fix` | off | Reset missing/corrupted files to pending for re-download |
| `--manifest <ID>` | all | Limit verification to a specific manifest |

## Thumbnails

```bash
# Generate thumbnails for all downloaded images
rustenati thumbnail

# Custom dimensions
rustenati thumbnail --width 300 --height 300

# Regenerate all (including existing)
rustenati thumbnail --force

# Only for a specific manifest
rustenati thumbnail --manifest <MANIFEST_ID>

# Adjust JPEG quality
rustenati thumbnail --quality 60
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `-W, --width` | 200 | Maximum thumbnail width in pixels |
| `-H, --height` | 200 | Maximum thumbnail height in pixels |
| `--quality` | 80 | JPEG quality (1-100) |
| `--manifest <ID>` | all | Only process a specific manifest |
| `--force` | off | Regenerate existing thumbnails |

Thumbnails are saved in a `thumbnails/` directory alongside `images/` in each registry folder.

## Dashboard

Interactive TUI for real-time monitoring of download progress and database statistics.

```bash
# Launch the dashboard
rustenati dashboard

# Custom refresh interval
rustenati dashboard --refresh 5
```

| Parameter | Default | Description |
|-----------|---------|-------------|
| `--refresh` | 2 | Refresh interval in seconds |

The dashboard displays:
- **Overview**: manifest count, archives, registries, downloads (complete/pending/failed), tags, OCR results, disk usage
- **Progress gauge**: visual download completion percentage
- **Recent manifests table**: latest manifests with doc type, year, status, and per-manifest progress

**Keys**: `q` or `Esc` to quit, `r` to force refresh.

## Configuration

```bash
rustenati config init     # create default config file
rustenati config show     # show current configuration
rustenati config set download.concurrency 8     # change a value
rustenati config set ocr.default_backend claude
```

Config file: `~/.config/rustenati/config.toml` (see [config.example.toml](config.example.toml))

## Source Formats

The `download` and `info` commands accept multiple source formats:

| Format | Example |
|--------|---------|
| Manifest URL | `https://dam-antenati.cultura.gov.it/antenati/containers/{uuid}/manifest` |
| Container UUID | `e3d78b31-0062-498d-9d76-f8379407d57f` |
| ARK identifier | `ark:/12657/an_ud12345` |
| Gallery URL | `https://antenati.cultura.gov.it/ark:/12657/an_ud12345` |

## Output Structure

All data is saved in the `./antenati` directory (fixed, not configurable). This includes downloaded images, OCR results, and the SQLite database that tracks all state.

```
antenati/
├── rustenati.db            # SQLite state database
├── {archive}/{register}/
│   ├── manifest.json       # IIIF manifest
│   ├── metadata.json       # Download metadata (date, version, etc.)
│   ├── images/
│   │   ├── 001_pag. 1.jpg
│   │   ├── 002_pag. 2.jpg
│   │   └── ...
│   ├── thumbnails/
│   │   ├── 001_pag. 1.jpg
│   │   ├── 002_pag. 2.jpg
│   │   └── ...
│   └── ocr/
│       ├── 001_pag. 1.txt  # Plain text transcription
│       ├── 001_pag. 1.json # Structured tags (when --extract-tags)
│       └── ...
└── ...
```

## Web Interface

Browse all downloaded documents locally with a built-in web interface:

```bash
# Start the web server
rustenati serve

# Open browser automatically
rustenati serve --open

# Custom port
rustenati serve --port 3000
```

The web interface provides:
- **Dashboard** with download statistics
- **Browse** registries with filters (document type, year, archive, locality)
- **Image viewer** with zoom/pan and keyboard navigation
- **Person search** with linked records
- **Full-text OCR search** across all transcriptions

## Documentation

- [Architecture](docs/architecture.md) — module diagram, data flows, concurrency model
- [API Reference](docs/api-reference.md) — reverse-engineered Portale Antenati endpoints
- [OCR Backends](docs/ocr-backends.md) — comparison, accuracy benchmarks, integration details

## Building

```bash
cargo build --release
cargo test
cargo run -- --help
```

Requires Rust 2024 edition (1.85+). SQLite is bundled — no system dependencies.

## License

MIT
