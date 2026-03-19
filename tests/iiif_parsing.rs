use rustenati::client::iiif;
use rustenati::models::manifest::IiifVersion;

fn load_fixture(name: &str) -> serde_json::Value {
    let path = format!("tests/fixtures/{name}");
    let data = std::fs::read_to_string(&path).expect(&format!("Failed to load fixture: {path}"));
    serde_json::from_str(&data).expect(&format!("Failed to parse fixture JSON: {path}"))
}

#[test]
fn parse_v2_manifest_from_fixture() {
    let json = load_fixture("manifest_v2.json");
    let manifest = iiif::parse_manifest(&json).unwrap();

    assert_eq!(manifest.version, IiifVersion::V2);
    assert_eq!(manifest.id, "https://dam-antenati.cultura.gov.it/antenati/containers/e3b0c442/manifest");
    assert_eq!(manifest.label, "Stato civile napoleonico - Nati - 1807");
    assert_eq!(manifest.canvases.len(), 3);

    // Metadata
    assert_eq!(manifest.title(), "Nati - 1807");
    assert_eq!(manifest.doc_type(), Some("Nati"));
    assert!(manifest.archival_context().unwrap().contains("Camposano"));

    // Canvas details
    let c1 = &manifest.canvases[0];
    assert_eq!(c1.label, "pag. 1");
    assert_eq!(c1.width, 3204);
    assert_eq!(c1.height, 4613);
    assert!(c1.image_service.id.contains("AN_ua18771_pag001"));

    let c3 = &manifest.canvases[2];
    assert_eq!(c3.label, "pag. 3");

    // Image URL generation
    let url = c1.full_image_url("jpg");
    assert_eq!(
        url,
        "https://iiif-antenati.cultura.gov.it/iiif/2/AN_ua18771_pag001/full/pct:100/0/default.jpg"
    );
}

#[test]
fn parse_v3_manifest_from_fixture() {
    let json = load_fixture("manifest_v3.json");
    let manifest = iiif::parse_manifest(&json).unwrap();

    assert_eq!(manifest.version, IiifVersion::V3);
    assert_eq!(manifest.label, "Nati - 1820");
    assert_eq!(manifest.canvases.len(), 2);

    // Metadata (v3 format with language maps)
    assert_eq!(manifest.title(), "Nati - 1820");
    assert_eq!(manifest.doc_type(), Some("Nati"));
    assert!(manifest.archival_context().unwrap().contains("Viareggio"));

    // Canvas
    let c1 = &manifest.canvases[0];
    assert_eq!(c1.label, "pag. 1");
    assert_eq!(c1.width, 2480);
    assert_eq!(c1.height, 3508);
    assert!(c1.image_service.id.contains("AN_ua22345_pag001"));

    // v3 uses "max" instead of "pct:100"
    let url = c1.full_image_url("jpg");
    assert_eq!(
        url,
        "https://iiif-antenati.cultura.gov.it/iiif/3/AN_ua22345_pag001/full/max/0/default.jpg"
    );
}

#[test]
fn v2_scaled_image_url() {
    let json = load_fixture("manifest_v2.json");
    let manifest = iiif::parse_manifest(&json).unwrap();
    let canvas = &manifest.canvases[0];

    let url = canvas.scaled_image_url(1600, "jpg");
    // 1600/3204 * 100 = ~49.9%
    assert!(url.contains("/full/pct:50/0/default.jpg"));
}

#[test]
fn v3_scaled_image_url() {
    let json = load_fixture("manifest_v3.json");
    let manifest = iiif::parse_manifest(&json).unwrap();
    let canvas = &manifest.canvases[0];

    let url = canvas.scaled_image_url(800, "jpg");
    assert!(url.contains("/full/800,/0/default.jpg"));
}

#[test]
fn metadata_not_found_returns_none() {
    let json = load_fixture("manifest_v2.json");
    let manifest = iiif::parse_manifest(&json).unwrap();

    assert!(manifest.get_metadata("NonExistent").is_none());
}

#[test]
fn invalid_manifest_returns_error() {
    let json: serde_json::Value = serde_json::json!({"foo": "bar"});
    let result = iiif::parse_manifest(&json);
    assert!(result.is_err());
}
