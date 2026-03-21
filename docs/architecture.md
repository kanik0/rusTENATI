# Architettura Rustenati

## Diagramma dei Moduli

```
┌─────────────────────────────────────────────────────┐
│                      CLI (clap)                     │
│  search │ download │ info │ ocr │ tags │ status     │
└────────────────────┬────────────────────────────────┘
                     │
        ┌────────────┼────────────────┐
        ▼            ▼                ▼
┌──────────┐  ┌────────────┐  ┌────────────┐
│  Client  │  │  Download  │  │    OCR     │
│          │  │   Engine   │  │  Backends  │
│ antenati │  │            │  │            │
│ iiif     │  │ engine     │  │ (planned)  │
│ waf      │  │ state(SQL) │  │            │
│ rate_lim │  │ progress   │  │            │
└────┬─────┘  └─────┬──────┘  └─────┬──────┘
     │              │               │
     ▼              ▼               ▼
┌─────────────────────────────────────────┐
│              Models                      │
│  manifest │ search │ ark │ metadata      │
└─────────────────────────────────────────┘
     │              │
     ▼              ▼
┌──────────┐  ┌──────────┐
│  Config  │  │  Output  │
│  (TOML)  │  │  (files) │
└──────────┘  └──────────┘
```

## Comandi Implementati

### `search registry` (funzionante)
Cerca nei registri del portale per località, anno, tipologia.
```
rustenati search registry --locality Napoli --year-from 1810 --doc-type Nati
```

### `search name` (funzionante)
Cerca per cognome/nome nelle anagrafiche indicizzate.
```
rustenati search name --surname Rossi --name Mario --locality Napoli
```

### `download` (funzionante)
Scarica immagini da un singolo manifest o ARK URL.
```
rustenati download <source> --pages 1-5
```

### `download --search` (funzionante)
Batch download di tutti i registri trovati da una ricerca.
```
rustenati download --search --locality Napoli --year-from 1810 --doc-type Nati --max-registries 50
```

### `info` (funzionante)
Ispeziona metadati di un manifest IIIF.
```
rustenati info <manifest_url|ark_url>
```

### `config` (funzionante)
Gestione configurazione.
```
rustenati config show
rustenati config init
```

## Flusso Download

```
1. Utente specifica manifest URL / archive ID / ARK
2. AntenatiClient fetch manifest IIIF
3. Parser normalizza v2/v3 → IiifManifest unificato
4. DownloadEngine enumera Canvas dal manifest
5. Per ogni canvas (parallelo, Semaphore-limited):
   a. Costruisci URL immagine IIIF
   b. Rate limiter: attendi token
   c. HTTP GET con retry + backoff
   d. Se WAF challenge → risolvi e riprova
   e. Salva immagine su disco
   f. Calcola SHA256
   g. Aggiorna stato SQLite → complete
   h. (opzionale) Invia a OCR backend
   i. (opzionale) Estrai tag da risultato OCR
6. Aggiorna progress bar
7. Al termine: stampa summary
```

## Flusso Batch Download

```
1. Utente specifica parametri di ricerca (--search --locality X ...)
2. Client cerca tutti i registri corrispondenti (paginato)
3. Per ogni registro trovato:
   a. Risolvi ARK → manifest URL (fetch pagina HTML, estrai manifest)
   b. Fetch manifest IIIF
   c. Download tutte le immagini con engine standard
4. Report finale: registri completati/falliti, immagini totali
```

## Flusso Ricerca

```
1. Utente specifica parametri di ricerca
2. AntenatiClient fetch pagina HTML risultati
3. Parser HTML (scraper) estrae risultati dalla pagina
4. Output tabellare (comfy-table) o JSON
5. (opzionale) Utente seleziona risultato → download
```

## Gestione Errori e Retry

Architettura a due livelli per gestire errori transitori del server (503, 429, 5xx):

### Livello 1: Client API (`get_with_retry`)
Tutte le chiamate HTTP in `AntenatiClient` (search, info, browse, suggest, manifest)
passano attraverso `get_with_retry()`, che gestisce automaticamente:
- **HTTP 503 / 5xx**: retry con backoff esponenziale + jitter (±25%)
- **HTTP 429**: retry con rispetto dell'header `Retry-After`
- **Header `Retry-After`**: parsato sia da 503 che da 429, usato come floor per il wait
- **Configurabile**: `api_max_retries` (default 3), `api_initial_backoff_ms` (default 1000ms)
- **Backoff**: raddoppia ogni tentativo, cap a 30 secondi
- **Non ritentati**: errori client (4xx eccetto 429), WAF challenges (202/405)

### Livello 2: Download Engine (`download_with_retry`)
Il download di immagini ha il proprio loop di retry separato che gestisce:
- Retry su errori di rete, timeout, 5xx, 429
- WAF challenge detection e risoluzione
- Circuit breaker per-dominio (5 fallimenti → cooldown 10s)
- Streaming file e checksum
- Cancellation token per graceful shutdown

I due livelli sono complementari: il livello 1 protegge le chiamate API (ricerche, manifest),
il livello 2 protegge i download di immagini con logica specifica.

## Concorrenza

- **tokio::sync::Semaphore** - limita download paralleli (default 8)
- **governor::RateLimiter** - token bucket globale (req/sec)
- **Backoff esponenziale** - retry con jitter su errori transitori (due livelli)
- **Circuit Breaker** - protezione per-dominio nel download engine

## Persistenza

### SQLite (rustenati.db)
- `downloads` - stato di ogni immagine scaricata (pending/complete/failed)
- `manifests` - cache manifesti IIIF
- `sessions` - sessioni di download
- `tags` - tag estratti da OCR (cognomi, nomi, date, ...)

### Filesystem
```
output/{archive}/{register}/
├── manifest.json       # Manifest IIIF completo
├── metadata.json       # Metadati download (data, versione, etc.)
├── images/
│   ├── 001_pag. 1.jpg
│   ├── 002_pag. 2.jpg
│   └── ...
└── ocr/                # (reserved for OCR results)
```
