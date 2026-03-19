use serde::{Deserialize, Serialize};

/// Unified IIIF manifest representation (normalized from v2 or v3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IiifManifest {
    pub id: String,
    pub label: String,
    pub metadata: Vec<MetadataEntry>,
    pub canvases: Vec<Canvas>,
    pub version: IiifVersion,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IiifVersion {
    V2,
    V3,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetadataEntry {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Canvas {
    pub id: String,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub image_service: ImageService,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImageService {
    /// Base URL for the IIIF Image API (e.g., `https://iiif-antenati.cultura.gov.it/iiif/2/{id}`)
    pub id: String,
    pub profile: String,
    pub version: IiifVersion,
}

impl Canvas {
    /// Construct the full-resolution image download URL.
    pub fn full_image_url(&self, format: &str) -> String {
        let size = match self.image_service.version {
            IiifVersion::V2 => "pct:100",
            IiifVersion::V3 => "max",
        };
        format!(
            "{}/full/{}/0/default.{}",
            self.image_service.id, size, format
        )
    }

    /// Construct a scaled image URL (percentage for v2, max width for v3).
    pub fn scaled_image_url(&self, max_width: u32, format: &str) -> String {
        match self.image_service.version {
            IiifVersion::V2 => {
                let pct = (max_width as f64 / self.width as f64 * 100.0).min(100.0);
                format!(
                    "{}/full/pct:{:.0}/0/default.{}",
                    self.image_service.id, pct, format
                )
            }
            IiifVersion::V3 => {
                format!(
                    "{}/full/{},/0/default.{}",
                    self.image_service.id, max_width, format
                )
            }
        }
    }
}

impl IiifManifest {
    /// Get metadata value by label (case-insensitive).
    pub fn get_metadata(&self, label: &str) -> Option<&str> {
        self.metadata
            .iter()
            .find(|m| m.label.eq_ignore_ascii_case(label))
            .map(|m| m.value.as_str())
    }

    /// Get the document title.
    pub fn title(&self) -> &str {
        self.get_metadata("Titolo").unwrap_or(&self.label)
    }

    /// Get the document type (Nati, Morti, Matrimoni, etc.).
    pub fn doc_type(&self) -> Option<&str> {
        self.get_metadata("Tipologia")
    }

    /// Get the archival context.
    pub fn archival_context(&self) -> Option<&str> {
        self.get_metadata("Contesto archivistico")
    }
}

impl std::fmt::Display for IiifVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IiifVersion::V2 => write!(f, "v2"),
            IiifVersion::V3 => write!(f, "v3"),
        }
    }
}
