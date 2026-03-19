use serde::{Deserialize, Serialize};

/// An archive (Archivio di Stato) listed on the portal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchiveInfo {
    /// Display name (e.g., "Archivio di Stato di Lucca")
    pub name: String,
    /// URL slug (e.g., "archivio-di-stato-di-lucca")
    pub slug: String,
    /// Full URL on the portal
    pub url: String,
}

/// Parameters for searching registries (supports both locality and archive filtering).
#[derive(Debug, Default)]
pub struct RegistrySearchParams<'a> {
    pub locality: Option<&'a str>,
    pub archive_id: Option<&'a str>,
    pub archive_name: Option<&'a str>,
    pub year_from: Option<i32>,
    pub year_to: Option<i32>,
    pub doc_type: Option<&'a str>,
    pub page: u32,
    pub page_size: u32,
    pub sort: Option<&'a str>,
}

/// A single registry search result parsed from HTML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryResult {
    /// ARK URL (e.g., `https://antenati.cultura.gov.it/ark:/12657/an_ua18771`)
    pub ark_url: String,
    /// Year label (e.g., "1810")
    pub year: String,
    /// Document type (e.g., "Nati", "Morti", "Matrimoni, indice")
    pub doc_type: String,
    /// Current archival signature (e.g., "82.1422")
    pub signature: String,
    /// Archival context path (e.g., "Stato civile napoleonico > Camposano (provincia di Napoli)")
    pub context: String,
    /// Holding archive name
    pub archive: String,
    /// Archive URL on the portal
    pub archive_url: Option<String>,
}

/// Parsed search results page with pagination info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    /// Total number of results
    pub total: u32,
    /// Current page (1-based)
    pub current_page: u32,
    /// Total number of pages
    pub total_pages: u32,
    /// Results per page
    pub page_size: u32,
    /// Results on this page
    pub results: Vec<RegistryResult>,
}

/// A single name search result parsed from HTML.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameResult {
    /// Full name
    pub name: String,
    /// Detail URL (e.g., `/detail-nominative/?s_id=123`)
    pub detail_url: String,
    /// Birth info (place, year)
    pub birth_info: Option<String>,
    /// Death info (place, year)
    pub death_info: Option<String>,
    /// Related records (linked acts)
    pub records: Vec<LinkedRecord>,
}

/// A linked record (act) from a name search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LinkedRecord {
    /// Record type (e.g., "Atto di nascita")
    pub record_type: String,
    /// Date info
    pub date: Option<String>,
    /// ARK URL to the document
    pub ark_url: Option<String>,
}

/// Parsed name search results page.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NameSearchResults {
    pub total: u32,
    pub current_page: u32,
    pub total_pages: u32,
    pub page_size: u32,
    pub results: Vec<NameResult>,
}
