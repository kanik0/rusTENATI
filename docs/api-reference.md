# Portale Antenati - API Reference

Documentazione reverse-engineered degli endpoint del Portale Antenati.
Verificata il 2026-03-19 tramite analisi diretta del sito.

## Stack Tecnologico

- **Frontend:** WordPress + WPML (multilingual) + jQuery
- **Search backend:** Apache Solr (dietro WordPress, esposto come HTML)
- **Image serving:** IIIF v2 (dam-antenati) + IIIF v3 (iiif-antenati)
- **Viewer:** Mirador (custom build in `/wp-content/themes/antenati/js/mirador.min.js`)
- **Identifiers:** ARK (`ark:/12657/...`)
- **Protection:** AWS WAF, mandatory Referer header, reCAPTCHA
- **Analytics:** Google Analytics + Matomo (analytics-icar.cultura.gov.it)

## Domini

| Dominio | Funzione |
|---------|----------|
| `antenati.cultura.gov.it` | Portale principale, ricerca, pagine HTML |
| `dam-antenati.cultura.gov.it` | DAM, IIIF v2 manifesti e immagini |
| `iiif-antenati.cultura.gov.it` | IIIF v2 Image API (serve le immagini) |

## Autocomplete Località

```
GET https://antenati.cultura.gov.it/suggest/?campo=localita&localita={query}&tipologia=
```

Risposta: JSON array di stringhe con suggerimenti (e.g., `["Napoli", "Casalnuovo di Napoli", ...]`).

## Ricerca Registri (verificata)

```
GET https://antenati.cultura.gov.it/search-registry/
```

Parametri query:
| Parametro | Descrizione | Esempio |
|-----------|-------------|---------|
| `localita` | Nome località | `Napoli` |
| `anno` | Anno singolo | `1810` |
| `anno_da` | Anno inizio range | `1800` |
| `anno_a` | Anno fine range | `1820` |
| `tipologia` | Tipo documento | `Nati`, `Morti`, `Matrimoni` |
| `s_page` | Pagina (1-based) | `2` |
| `s_size` | Risultati per pagina | `10`, `20`, `50`, `100` |
| `s_sort` | Ordinamento | `estremoRemoto_i asc`, `estremoRemoto_i desc` |
| `lang` | Lingua | `it`, `en`, `es`, `fr`, `de`, `pt-pt` |
| `s_facet_query` | Filtri faccette Solr | `tipologia_ss:Nati` |

**Tipologie note:** Nati, Morti, Matrimoni, Matrimoni (indice), Matrimoni (processetti), Matrimoni (pubblicazioni), Nati (indice), Morti (indice), Diversi

La risposta è HTML. Struttura dei risultati:
```html
<ul class="no-appearance">
  <li class="search-item">
    <div>
      <h3 class="text-primary"><a href="/ark:/12657/an_ua18771">Registro: 1810</a></h3>
      <p>Morti, indice</p>
      <p>Segnatura attuale: 82.1422</p>
      <p>Stato civile napoleonico e della restaurazione > Camposano (provincia di Napoli)</p>
      <p>Conservato da: <a href="/archivio/archivio-di-stato-di-caserta">Archivio di Stato di Caserta</a></p>
    </div>
    <aside><a href="/ark:/12657/an_ua18771">Vedi il registro</a></aside>
  </li>
</ul>
```

Paginazione nel footer: `Pagina X di Y`, contatore: `<span>618</span> risultati`.

**Sidebar faccette** (filtri disponibili): Archivio, Fondo, Serie, Località, Tipologia, Anno.

## Ricerca Nominativa

```
GET https://antenati.cultura.gov.it/search-nominative/
```

Parametri:
| Parametro | Descrizione |
|-----------|-------------|
| `cognome` | Cognome |
| `nome` | Nome |
| `luogo_nascita` | Luogo di nascita |
| `luogo_morte` | Luogo di morte |
| `anno_nascita_da` / `anno_nascita_a` | Range anno nascita |
| `anno_morte_da` / `anno_morte_a` | Range anno morte |

(Da verificare più nel dettaglio)

## Pagina Registro (Gallery)

```
GET https://antenati.cultura.gov.it/ark:/12657/an_ua{id}
```

La pagina contiene un viewer Mirador che carica il manifest IIIF. Il manifest URL è incorporato nel JavaScript della pagina.

## IIIF Manifesti (verificato)

### Pattern DAM (usato dal viewer Mirador)
```
GET https://dam-antenati.cultura.gov.it/antenati/containers/{container_id}/manifest
```

**Richiede header `Referer: https://antenati.cultura.gov.it/`** (altrimenti 403).

Risposta: JSON IIIF Presentation API v2. Esempio struttura:
```json
{
  "@context": "http://iiif.io/api/presentation/2/context.json",
  "@id": "https://dam-antenati.cultura.gov.it/antenati/containers/{id}/manifest",
  "label": "Archivio > Fondo > Località > Anno",
  "metadata": [
    {"label": "Titolo", "value": "1810"},
    {"label": "Tipologia", "value": "Morti, indice"},
    {"label": "Datazione", "value": "1810/01/01 - 1810/12/31"},
    {"label": "Contesto archivistico", "value": "..."},
    {"label": "Conservato da", "value": "Archivio di Stato di ..."},
    {"label": "Licenza", "value": "<a href='...'>...</a>"},
    {"label": "Lingua", "value": "it"}
  ],
  "sequences": [{
    "canvases": [{
      "@id": "https://antenati.cultura.gov.it/ark:/12657/an_ua.../wXXXXXX",
      "width": 1000, "height": 1000,
      "images": [{
        "resource": {
          "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/{image_id}/full/full/0/default.jpg",
          "service": {
            "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/{image_id}",
            "profile": "http://iiif.io/api/image/2/level2.json"
          }
        }
      }]
    }]
  }]
}
```

## IIIF Image API (verificato)

```
GET https://iiif-antenati.cultura.gov.it/iiif/2/{image_id}/{region}/{size}/{rotation}/{quality}.{format}
```

Parametri:
- `region`: `full` o `{x},{y},{w},{h}`
- `size`: `full`, `pct:100`, `{w},`, `,{h}`, `{w},{h}`
- `rotation`: `0`
- `quality`: `default`
- `format`: `jpg`, `png`

Esempio full-resolution:
```
https://iiif-antenati.cultura.gov.it/iiif/2/wXN1dW5/full/pct:100/0/default.jpg
```

Esempio thumbnail (843px width, usato dal viewer):
```
https://iiif-antenati.cultura.gov.it/iiif/2/wXN1dW5/full/843,/0/default.jpg
```

## ARK Identifiers

Formato: `ark:/12657/an_{type}{id}`

Tipi:
- `ua` - Unità archivistica (registro/raccolta)
- `ud` - Unità documentaria

Esempio: `ark:/12657/an_ua18771` → pagina registro su portale

## Anti-Scraping

### Header obbligatorio
```
Referer: https://antenati.cultura.gov.it/
```
Senza questo header tutte le richieste vengono bloccate con 403.

### User-Agent
Richiesto un user-agent realistico (browser-like).

### Cookie Store
Il sito usa cookies per sessione. Un cookie store persistente è necessario.

### AWS WAF
- Può restituire challenge (HTTP 202/405 con body HTML/JavaScript)
- Il challenge genera un cookie `aws-waf-token`
- Il cookie va incluso nelle richieste successive
- Header `x-amzn-waf-action: challenge` indica WAF attivo

### reCAPTCHA
La pagina di ricerca include Google reCAPTCHA. Non è attivato per navigazione normale ma potrebbe scattare con traffico elevato.

### Best Practices
- Delay tra richieste: minimo 500ms
- Parallelismo: max 4-8 richieste contemporanee
- Cookie jar persistente
- User-Agent realistico (Chrome/Firefox)
- Rispettare 429 (retry-after)

## Numeri

- 132+ milioni di immagini
- 1.6 milioni di manifesti IIIF
- ~100 Archivi di Stato

## Riferimenti

- [gcerretani/antenati](https://github.com/gcerretani/antenati) - Python downloader
- [DanielePigoli/ATK-Pro-v2](https://github.com/DanielePigoli/ATK-Pro-v2) - Python, IIIF v2/v3
- [cantaprete/bradipo](https://github.com/cantaprete/bradipo) - Python, containers endpoint
- [LBreda/antenati-dl](https://github.com/LBreda/antenati-dl) - Node.js downloader
- [IIIF 2023 Naples Conference](https://iiif.io/event/2023/naples/schedule/) - Presentazione infrastruttura
