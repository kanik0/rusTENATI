use serde_json::Value;

use crate::error::RustenatiError;
use crate::models::manifest::{Canvas, IiifManifest, IiifVersion, ImageService, MetadataEntry};

/// Parse a raw IIIF manifest JSON into our unified model.
/// Automatically detects v2 vs v3 based on `@context` or `type` fields.
pub fn parse_manifest(json: &Value) -> Result<IiifManifest, RustenatiError> {
    let version = detect_version(json)?;
    match version {
        IiifVersion::V2 => parse_v2(json),
        IiifVersion::V3 => parse_v3(json),
    }
}

fn detect_version(json: &Value) -> Result<IiifVersion, RustenatiError> {
    // v2: has "@context" containing "presentation/2"
    if let Some(ctx) = json.get("@context") {
        let ctx_str = match ctx {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr
                .iter()
                .filter_map(|v| v.as_str())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        };
        if ctx_str.contains("presentation/2") {
            return Ok(IiifVersion::V2);
        }
        if ctx_str.contains("presentation/3") {
            return Ok(IiifVersion::V3);
        }
    }

    // v3: has "type": "Manifest"
    if json.get("type").and_then(|v| v.as_str()) == Some("Manifest") {
        return Ok(IiifVersion::V3);
    }

    // v2: has "@type": "sc:Manifest"
    if json.get("@type").and_then(|v| v.as_str()) == Some("sc:Manifest") {
        return Ok(IiifVersion::V2);
    }

    Err(RustenatiError::ManifestParse(
        "Cannot determine IIIF version from manifest".into(),
    ))
}

fn parse_v2(json: &Value) -> Result<IiifManifest, RustenatiError> {
    let id = json
        .get("@id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let label = json
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let metadata = parse_metadata_v2(json.get("metadata"));

    let canvases = json
        .get("sequences")
        .and_then(|s| s.as_array())
        .and_then(|seqs| seqs.first())
        .and_then(|seq| seq.get("canvases"))
        .and_then(|c| c.as_array())
        .map(|canvases| {
            canvases
                .iter()
                .filter_map(|c| parse_canvas_v2(c).ok())
                .collect()
        })
        .unwrap_or_default();

    Ok(IiifManifest {
        id,
        label,
        metadata,
        canvases,
        version: IiifVersion::V2,
    })
}

fn parse_canvas_v2(canvas: &Value) -> Result<Canvas, RustenatiError> {
    let id = canvas
        .get("@id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let label = canvas
        .get("label")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let width = canvas
        .get("width")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let height = canvas
        .get("height")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // Navigate: images[0].resource.service
    let image_service = canvas
        .get("images")
        .and_then(|i| i.as_array())
        .and_then(|imgs| imgs.first())
        .and_then(|img| img.get("resource"))
        .and_then(|res| {
            let service = res.get("service")?;
            let service_id = service.get("@id").and_then(|v| v.as_str())?.to_string();
            let profile = service
                .get("profile")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(ImageService {
                id: service_id,
                profile,
                version: IiifVersion::V2,
            })
        })
        .ok_or_else(|| {
            RustenatiError::ManifestParse(format!("No image service found for canvas {id}"))
        })?;

    Ok(Canvas {
        id,
        label,
        width,
        height,
        image_service,
    })
}

fn parse_metadata_v2(metadata: Option<&Value>) -> Vec<MetadataEntry> {
    metadata
        .and_then(|m| m.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let label = entry.get("label").and_then(|v| v.as_str())?.to_string();
                    let value = entry.get("value").and_then(|v| v.as_str())?.to_string();
                    Some(MetadataEntry { label, value })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn parse_v3(json: &Value) -> Result<IiifManifest, RustenatiError> {
    let id = json
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let label = extract_v3_label(json.get("label"));

    let metadata = parse_metadata_v3(json.get("metadata"));

    let canvases = json
        .get("items")
        .and_then(|items| items.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| parse_canvas_v3(item).ok())
                .collect()
        })
        .unwrap_or_default();

    Ok(IiifManifest {
        id,
        label,
        metadata,
        canvases,
        version: IiifVersion::V3,
    })
}

fn parse_canvas_v3(canvas: &Value) -> Result<Canvas, RustenatiError> {
    let id = canvas
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let label = extract_v3_label(canvas.get("label"));

    let width = canvas
        .get("width")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    let height = canvas
        .get("height")
        .and_then(|v| v.as_u64())
        .unwrap_or(0) as u32;

    // Navigate: items[0].items[0].body.service[0]
    let image_service = canvas
        .get("items")
        .and_then(|i| i.as_array())
        .and_then(|pages| pages.first())
        .and_then(|page| page.get("items"))
        .and_then(|i| i.as_array())
        .and_then(|annos| annos.first())
        .and_then(|anno| anno.get("body"))
        .and_then(|body| {
            let service = body.get("service")?.as_array()?.first()?;
            let service_id = service.get("id").and_then(|v| v.as_str())?.to_string();
            let profile = service
                .get("profile")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            Some(ImageService {
                id: service_id,
                profile,
                version: IiifVersion::V3,
            })
        })
        .ok_or_else(|| {
            RustenatiError::ManifestParse(format!("No image service found for canvas {id}"))
        })?;

    Ok(Canvas {
        id,
        label,
        width,
        height,
        image_service,
    })
}

/// Extract a label from IIIF v3 format: `{"it": ["value"]}` or `{"none": ["value"]}`.
fn extract_v3_label(label: Option<&Value>) -> String {
    label
        .and_then(|l| {
            if let Some(obj) = l.as_object() {
                // Try "it" first, then "none", then first available
                let values = obj
                    .get("it")
                    .or_else(|| obj.get("none"))
                    .or_else(|| obj.values().next());
                values
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
            } else {
                l.as_str().map(|s| s.to_string())
            }
        })
        .unwrap_or_default()
}

fn parse_metadata_v3(metadata: Option<&Value>) -> Vec<MetadataEntry> {
    metadata
        .and_then(|m| m.as_array())
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let label = extract_v3_label(entry.get("label"));
                    let value = extract_v3_label(entry.get("value"));
                    if label.is_empty() {
                        return None;
                    }
                    Some(MetadataEntry { label, value })
                })
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_v2_manifest() {
        let json: Value = serde_json::from_str(V2_FIXTURE).unwrap();
        let manifest = parse_manifest(&json).unwrap();

        assert_eq!(manifest.version, IiifVersion::V2);
        assert_eq!(manifest.label, "Stato civile - Nati - 1807");
        assert_eq!(manifest.canvases.len(), 2);

        let canvas = &manifest.canvases[0];
        assert_eq!(canvas.label, "pag. 1");
        assert_eq!(canvas.width, 3000);
        assert_eq!(canvas.height, 4000);
        assert!(canvas
            .image_service
            .id
            .contains("iiif-antenati.cultura.gov.it"));

        let url = canvas.full_image_url("jpg");
        assert!(url.ends_with("/full/pct:100/0/default.jpg"));
    }

    #[test]
    fn parse_v3_manifest() {
        let json: Value = serde_json::from_str(V3_FIXTURE).unwrap();
        let manifest = parse_manifest(&json).unwrap();

        assert_eq!(manifest.version, IiifVersion::V3);
        assert_eq!(manifest.label, "Nati - 1820");
        assert_eq!(manifest.canvases.len(), 1);

        let canvas = &manifest.canvases[0];
        assert_eq!(canvas.label, "pag. 1");

        let url = canvas.full_image_url("jpg");
        assert!(url.ends_with("/full/max/0/default.jpg"));
    }

    #[test]
    fn metadata_accessors() {
        let json: Value = serde_json::from_str(V2_FIXTURE).unwrap();
        let manifest = parse_manifest(&json).unwrap();

        assert_eq!(manifest.title(), "Nati - 1807");
        assert_eq!(manifest.doc_type(), Some("Nati"));
        assert!(manifest.archival_context().is_some());
    }

    const V2_FIXTURE: &str = r#"{
        "@context": "http://iiif.io/api/presentation/2/context.json",
        "@id": "https://dam-antenati.cultura.gov.it/antenati/containers/abc123/manifest",
        "@type": "sc:Manifest",
        "label": "Stato civile - Nati - 1807",
        "metadata": [
            {"label": "Contesto archivistico", "value": "Archivio di Stato di Lucca - Viareggio"},
            {"label": "Titolo", "value": "Nati - 1807"},
            {"label": "Tipologia", "value": "Nati"}
        ],
        "sequences": [{
            "@type": "sc:Sequence",
            "canvases": [
                {
                    "@id": "https://dam-antenati.cultura.gov.it/iiif/2/img001",
                    "@type": "sc:Canvas",
                    "label": "pag. 1",
                    "width": 3000,
                    "height": 4000,
                    "images": [{
                        "@type": "oa:Annotation",
                        "motivation": "sc:painting",
                        "resource": {
                            "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/img001/full/full/0/default.jpg",
                            "@type": "dctypes:Image",
                            "service": {
                                "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/img001",
                                "@context": "http://iiif.io/api/image/2/context.json",
                                "profile": "http://iiif.io/api/image/2/level2.json"
                            }
                        },
                        "on": "https://dam-antenati.cultura.gov.it/iiif/2/img001"
                    }]
                },
                {
                    "@id": "https://dam-antenati.cultura.gov.it/iiif/2/img002",
                    "@type": "sc:Canvas",
                    "label": "pag. 2",
                    "width": 3000,
                    "height": 4000,
                    "images": [{
                        "@type": "oa:Annotation",
                        "motivation": "sc:painting",
                        "resource": {
                            "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/img002/full/full/0/default.jpg",
                            "@type": "dctypes:Image",
                            "service": {
                                "@id": "https://iiif-antenati.cultura.gov.it/iiif/2/img002",
                                "@context": "http://iiif.io/api/image/2/context.json",
                                "profile": "http://iiif.io/api/image/2/level2.json"
                            }
                        },
                        "on": "https://dam-antenati.cultura.gov.it/iiif/2/img002"
                    }]
                }
            ]
        }]
    }"#;

    const V3_FIXTURE: &str = r#"{
        "@context": "http://iiif.io/api/presentation/3/context.json",
        "id": "https://example.org/manifest.json",
        "type": "Manifest",
        "label": {"it": ["Nati - 1820"]},
        "metadata": [
            {"label": {"it": ["Titolo"]}, "value": {"it": ["Nati - 1820"]}},
            {"label": {"it": ["Tipologia"]}, "value": {"it": ["Nati"]}}
        ],
        "items": [{
            "id": "https://example.org/canvas/p1",
            "type": "Canvas",
            "label": {"it": ["pag. 1"]},
            "width": 3204,
            "height": 4613,
            "items": [{
                "id": "https://example.org/page/p1/1",
                "type": "AnnotationPage",
                "items": [{
                    "id": "https://example.org/annotation/p0001-image",
                    "type": "Annotation",
                    "motivation": "painting",
                    "body": {
                        "id": "https://iiif-antenati.cultura.gov.it/iiif/3/img001/full/max/0/default.jpg",
                        "type": "Image",
                        "format": "image/jpeg",
                        "service": [{
                            "id": "https://iiif-antenati.cultura.gov.it/iiif/3/img001",
                            "type": "ImageService3",
                            "profile": "level1"
                        }]
                    },
                    "target": "https://example.org/canvas/p1"
                }]
            }]
        }]
    }"#;
}
