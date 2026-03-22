# OCR Backends - Stato dell'Arte 2026

## Panoramica

I documenti del Portale Antenati sono principalmente manoscritti in corsivo italiano del XVI-XIX secolo.
La difficoltà principale è il riconoscimento di scrittura a mano storica con abbreviazioni, fioriture e stili regionali.

## Backend Supportati

### 1. Transkribus (Priorità: Alta)

**Il leader per manoscritti storici italiani.**

- Modello: "Italian Handwriting M1" (XVI-XIX secolo)
- CER (Character Error Rate): ~12% su manoscritti complessi
- Archivio Storico Ricordi (Milano): CER 12.3% su 88.000 parole
- Supporta training di modelli custom sui propri documenti
- API a sottoscrizione (crediti)

**Integrazione:** REST API con upload immagine, selezione modello, polling risultato.

**Formati output:** Testo piano, ALTO XML, PAGE XML

### 2. Claude Vision (Priorità: Alta)

**Ottima comprensione contestuale dei documenti.**

- WER (Word Error Rate): 4.2% su documenti scritti a mano
- Eccelle nella comprensione del contesto e layout
- Non specificamente trainato su corsivo storico italiano
- Prompt engineering importante per risultati ottimali

**Integrazione:** Anthropic API, immagine base64-encoded, prompt specializzato.

**Prompt consigliato:**
```
Sei un esperto paleografo italiano. Trascrivi il testo manoscritto in questa immagine.
Il documento è un atto di [nascita/morte/matrimonio] del [XIX secolo].
Trascrivi fedelmente, indicando con [?] le parole incerte.
Restituisci anche un JSON strutturato con: cognomi, nomi, date, località, tipo_evento, ruoli.
```

### 3. Azure Document Intelligence (Priorità: Media)

- Supporto esplicito per italiano scritto a mano
- Modello `prebuilt-read` con language hint `it`
- Buone prestazioni su scrittura semi-leggibile

**Integrazione:** REST API, upload immagine, risultato sincrono o asincrono.

### 4. Google Cloud Vision (Priorità: Media)

- Multilingual incluso italiano
- `TEXT_DETECTION` + `DOCUMENT_TEXT_DETECTION`
- Language hint per ottimizzare accuratezza

**Integrazione:** REST API, immagine base64 o GCS URI.

### 5. VLM Open-Source (Priorità: Futura)

Per inferenza locale senza API:
- **Qwen2-VL** (2B/7B/72B) - 90+ lingue incluso italiano
- **InternVL 3** - Potente VLM open-source
- **OlmOCR-2** - Allen AI, ottimizzato per documenti
- **Kraken** - HTR classico per documenti storici

**Nota:** I modelli open-source degradano significativamente su documenti storici non inglesi rispetto ai modelli proprietari.

## Confronto

| Backend | Accuratezza storico IT | Costo | Velocità | Offline |
|---------|----------------------|-------|----------|---------|
| Transkribus | ★★★★★ | €€ | Media | No |
| Claude Vision | ★★★★ | €€ | Media | No |
| Azure | ★★★ | €€ | Veloce | No |
| Google | ★★★ | €€ | Veloce | No |
| VLM locale | ★★ | Gratis | Lenta | Sì |

## Architettura Trait

```rust
#[async_trait]
pub trait OcrBackend: Send + Sync {
    fn name(&self) -> &str;
    async fn recognize(
        &self,
        image_path: &Path,
        language: &str,
        doc_type: DocumentType,
        extract_tags: bool,
    ) -> Result<OcrResult>;
}
```

## Image Enhancement Pre-OCR

Rustenati include un pipeline di miglioramento immagini opzionale (`--enhance`) che migliora l'accuratezza OCR del 20-40% su documenti degradati:

| Stadio | Tecnica | Effetto |
|--------|---------|---------|
| 1. Contrasto | Histogram stretching (min-max) | Migliora inchiostro sbiadito |
| 2. Denoising | Median filter 3x3 | Riduce grana carta e artefatti |
| 3. Binarizzazione | Otsu threshold (opzionale) | Converte in bianco/nero |

```bash
# Attivare il miglioramento
rustenati ocr ./images/ --backend claude --enhance

# Aggiungere binarizzazione aggressiva (utile per documenti molto degradati)
rustenati ocr ./images/ --backend claude --enhance --binarize
```

**Quando usare `--enhance`:**
- Documenti con inchiostro sbiadito o poco contrasto
- Fotografie con illuminazione non uniforme
- Scansioni con sfondo rumoroso o macchie
- Documenti del XVIII-XIX secolo particolarmente deteriorati

**Quando NON usare `--binarize`:**
- Documenti già in buone condizioni (può peggiorare)
- Immagini con gradazioni tonali importanti
- Quando il testo è molto chiaro su sfondo scuro

## Estrazione Tag Strutturati

Dopo la trascrizione, il testo viene analizzato per estrarre:
- Cognomi e nomi delle persone menzionate
- Date (normalizzate ISO 8601)
- Località
- Tipo evento (nascita, morte, matrimonio, battesimo)
- Ruoli (padre, madre, testimone, ufficiale)
- Professioni

Per Claude Vision, l'estrazione può avvenire nel prompt stesso chiedendo output JSON strutturato.
Per altri backend, serve un post-processing NLP o un secondo passaggio con LLM.

## Knowledge Graph da OCR

I tag estratti dall'OCR vengono utilizzati per costruire automaticamente un grafo di relazioni familiari:

```bash
rustenati graph build    # processa tutti i tag OCR → grafo
rustenati graph query "Rossi"
rustenati graph ancestors 42
rustenati graph export --format dot | dot -Tsvg -o family.svg
```

Il grafo inferisce relazioni dai tipi di documento:
- **Atti di nascita**: identifica genitori → relazione `parent_of`
- **Atti di matrimonio**: identifica coniugi → relazione `spouse_of`
- **Atti di morte**: identifica coniuge → relazione `spouse_of`
- **Tutti gli atti**: identifica testimoni → relazione `witness_for`
