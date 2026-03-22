# Architettura Rustenati

## Diagramma dei Moduli

```
┌──────────────────────────────────────────────────────────────────┐
│                           CLI (clap)                             │
│  search │ browse │ download │ info │ ocr │ tags │ ask │ graph   │
│  status │ config │ query │ serve │ verify │ export │ sync       │
│  thumbnail │ link │ dashboard                                    │
└─────────────────────┬────────────────────────────────────────────┘
                     │
        ┌────────────┼────────────────┐
        ▼            ▼                ▼
┌──────────────┐  ┌────────────┐  ┌────────────────┐
│    Client    │  │  Download  │  │      OCR       │
│              │  │   Engine   │  │   Backends     │
│ antenati     │  │            │  │                │
│ iiif         │  │ engine     │  │ claude_vision  │
│ waf          │  │ state(SQL) │  │ transkribus    │
│ per_host_lim │  │ progress   │  │ azure          │
│ rate_limiter │  │ adaptive   │  │ google         │
│              │  │ buffer_pool│  │ enhance        │
└────┬─────────┘  └─────┬──────┘  └─────┬──────────┘
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
Batch download di tutti i registri trovati da una ricerca. Supporta anteprima, conferma, campionamento e ordinamento.
```
rustenati download --search --locality Napoli --year-from 1810 --doc-type Nati --max-registries 50
rustenati download --search --locality Napoli --count          # solo conteggio
rustenati download --search --locality Napoli --sample 5       # campione casuale
rustenati download --search --locality Napoli --sort-by year   # ordina per anno
```

### `info` (funzionante)
Ispeziona metadati di un manifest IIIF.
```
rustenati info <manifest_url|ark_url>
```

### `ocr` (funzionante)
Trascrivi immagini con OCR, opzionalmente con miglioramento immagini pre-OCR.
```
rustenati ocr ./images/ --backend claude --extract-tags
rustenati ocr ./images/ --backend claude --enhance              # con miglioramento
rustenati ocr ./images/ --backend claude --enhance --binarize   # binarizzazione aggressiva
```

### `ask` (funzionante)
Assistente genealogico AI: domande in linguaggio naturale sui documenti trascritti.
```
rustenati ask "Chi erano i genitori di Giuseppe Rossi?"
rustenati ask "Quanti matrimoni ci sono nel 1845?" --context 20
```

### `graph` (funzionante)
Grafo delle relazioni familiari: costruzione, ricerca, visualizzazione.
```
rustenati graph build                      # costruisci il grafo dai tag OCR
rustenati graph query "Rossi"              # cerca persona e relazioni
rustenati graph ancestors 42               # antenati (BFS)
rustenati graph export --format dot        # esporta Graphviz DOT
rustenati graph stats                      # statistiche del grafo
```

### `export` (funzionante)
Esporta dati in CSV, JSON o GEDCOM 5.5.1.
```
rustenati export --type csv
rustenati export --type json
rustenati export --type gedcom             # standard universale genealogia
```

### `sync` (funzionante)
Sync incrementale: verifica aggiornamenti sul portale tramite ETag/Last-Modified.
```
rustenati sync                             # controlla tutti i manifesti
rustenati sync --older-than-days 30        # solo manifesti vecchi 30+ giorni
rustenati sync --dry-run                   # solo report, senza aggiornare
```

### `config` (funzionante)
Gestione configurazione. Validazione automatica all'avvio con messaggi chiari per valori invalidi.
```
rustenati config show
rustenati config init
```

### `verify` (funzionante)
Verifica l'integrità dei file scaricati tramite SHA256.
```
rustenati verify                      # verifica completa
rustenati verify --quick              # solo esistenza + dimensione
rustenati verify --fix                # riaccoda file corrotti per ri-download
```

### `thumbnail` (funzionante)
Genera miniature JPEG dai file immagine scaricati.
```
rustenati thumbnail                   # genera miniature (200x200)
rustenati thumbnail --width 300 --height 300 --quality 60
```

### `dashboard` (funzionante)
Dashboard TUI interattiva per monitorare download e statistiche in tempo reale.
```
rustenati dashboard                   # avvia la dashboard
rustenati dashboard --refresh 5       # intervallo di aggiornamento personalizzato
```

## Flusso Download

```
1. Utente specifica manifest URL / archive ID / ARK
2. AntenatiClient fetch manifest IIIF
3. Parser normalizza v2/v3 → IiifManifest unificato
4. DownloadEngine enumera Canvas dal manifest
5. Pre-scan directory output: costruisci HashSet dei file esistenti
6. Per ogni canvas (parallelo, Semaphore-limited):
   a. Se file esiste in HashSet → skip (zero syscall)
   b. Costruisci URL immagine IIIF
   c. Per-host rate limiter: attendi token (budget separato per dominio)
   d. HTTP GET con retry + backoff
   e. Se WAF challenge → risolvi e riprova
   f. Salva immagine su disco
   g. Calcola SHA256
   h. Aggiorna stato SQLite → complete (bulk INSERT in transazione)
   i. (opzionale) Invia a OCR backend
   j. (opzionale) Estrai tag da risultato OCR
7. Aggiorna progress bar
8. Al termine: stampa summary
```

## Flusso Batch Download

```
1. Utente specifica parametri di ricerca (--search --locality X ...)
2. Client cerca tutti i registri corrispondenti (paginato)
3. Anteprima batch: mostra conteggio registri, immagini stimate
4. (opzionale) Applica --sort-by, --sample, --count
5. Conferma utente (o --yes per skip)
6. Per ogni registro trovato:
   a. Risolvi ARK → manifest URL (fetch pagina HTML, estrai manifest)
   b. Fetch manifest IIIF
   c. Download tutte le immagini con engine standard
7. Report finale: registri completati/falliti, immagini totali
```

## Flusso AI Assistant (`ask`)

```
1. Utente pone una domanda in linguaggio naturale
2. Parsing della domanda: estrae nomi, cognomi, date, parole chiave
3. Query FTS5 sul database OCR locale
4. Recupero top-K risultati OCR con metadata (manifest, documento)
5. Costruzione prompt di sistema con contesto documenti
6. Chiamata Claude API con streaming SSE
7. Risposta in tempo reale a stdout
```

## Flusso Knowledge Graph

```
1. `graph build` processa tutti i risultati OCR con tag
2. Per ogni documento OCR:
   a. Raggruppa tag per documento sorgente
   b. Identifica persone (cognome + nome)
   c. Crea nodi nel grafo (upsert per evitare duplicati)
   d. Inferisci relazioni dall'evento:
      - Nascita → genitore-di
      - Matrimonio → coniuge-di
      - Morte → coniuge-di
      - Tutti → testimone-per
   e. Crea archi nel grafo
3. `graph query` cerca nodi per nome e mostra relazioni
4. `graph ancestors` BFS sugli archi genitore
5. `graph export` genera DOT (Graphviz) o JSON
```

## Flusso Sync Incrementale

```
1. Carica tutti i manifesti dal database con i loro ETag/Last-Modified
2. (opzionale) Filtra per età (--older-than-days)
3. Per ogni manifesto:
   a. HTTP GET condizionale con If-None-Match / If-Modified-Since
   b. 304 Not Modified → nessun cambio
   c. 200 OK → manifest aggiornato, salva nuovi headers
   d. Confronta total_canvases vecchio vs nuovo
4. Report: aggiornati, invariati, errori
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
- **PerHostLimiter** - rate limiting separato per dominio (dam-antenati, iiif-antenati)
- **AIMD Adaptive** - aggiustamento dinamico della concorrenza basato su successi/fallimenti
- **Backoff esponenziale** - retry con jitter su errori transitori (due livelli)
- **Circuit Breaker** - protezione per-dominio nel download engine

## Persistenza

### SQLite (rustenati.db)
- `downloads` - stato di ogni immagine scaricata (pending/complete/failed)
- `manifests` - cache manifesti IIIF con ETag/Last-Modified per sync incrementale
- `sessions` - sessioni di download
- `tags` - tag estratti da OCR (cognomi, nomi, date, ...)
- `registries` - catalogo persistente dei risultati di ricerca
- `graph_nodes` / `graph_edges` - grafo relazioni familiari
- `ocr_fulltext` - ricerca full-text FTS5 sui risultati OCR
- **Schema versioning** con migrazioni (attuale: v6)

### Filesystem
```
output/{archive}/{register}/
├── manifest.json       # Manifest IIIF completo
├── metadata.json       # Metadati download (data, versione, etc.)
├── images/
│   ├── 001_pag. 1.jpg
│   ├── 002_pag. 2.jpg
│   └── ...
├── thumbnails/
│   ├── 001_pag. 1.jpg
│   └── ...
└── ocr/
    ├── 001_pag. 1.txt  # Trascrizione testo piano
    ├── 001_pag. 1.json # Tag strutturati (quando --extract-tags)
    └── ...
```

## Image Enhancement Pipeline

Pipeline di pre-processing opzionale per migliorare l'accuratezza OCR su documenti degradati:

```
Immagine originale
    │
    ▼
┌──────────────────┐
│ Contrast stretch │  Normalizzazione istogramma min-max
└────────┬─────────┘
         ▼
┌──────────────────┐
│  Median filter   │  Filtro mediano 3x3 per riduzione rumore
└────────┬─────────┘
         ▼
┌──────────────────┐
│ Otsu binarize    │  (opzionale, --binarize) Soglia automatica
└────────┬─────────┘
         ▼
    Immagine enhanced → OCR backend
```

Attivabile con `--enhance` nel comando `ocr`. Migliora l'accuratezza del 20-40% su documenti con inchiostro sbiadito, basso contrasto o sfondo rumoroso.
