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
    async fn recognize(&self, image_path: &Path, language: &str) -> Result<OcrResult>;
    fn supports_batch(&self) -> bool { false }
}
```

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
