use std::fmt;

use crate::error::RustenatiError;

/// Type of ARK identifier used by Portale Antenati.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArkType {
    /// Single act (atto unico) - `an_ua{id}`
    Act,
    /// Register with multiple pages (unità documentaria) - `an_ud{id}`
    Register,
}

/// Parsed ARK persistent identifier.
///
/// Format: `ark:/12657/an_{type}{id}`
/// Where type is `ua` (single act) or `ud` (register).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArkIdentifier {
    pub naan: String,
    pub ark_type: ArkType,
    pub id: String,
}

impl ArkIdentifier {
    /// Parse an ARK identifier string.
    ///
    /// Accepts formats:
    /// - `ark:/12657/an_ua12345`
    /// - `ark:/12657/an_ud12345`
    /// - Full URL: `https://antenati.cultura.gov.it/ark:/12657/an_ua12345`
    pub fn parse(input: &str) -> Result<Self, RustenatiError> {
        let ark_part = if let Some(idx) = input.find("ark:/") {
            &input[idx..]
        } else {
            return Err(RustenatiError::InvalidArk(input.to_string()));
        };

        // ark:/12657/an_ua12345 or ark:/12657/an_ud12345
        let parts: Vec<&str> = ark_part.splitn(3, '/').collect();
        if parts.len() < 3 {
            return Err(RustenatiError::InvalidArk(input.to_string()));
        }

        let naan = parts[1].to_string();

        // The qualifier may have a trailing path segment (e.g., an_ua12345/00001)
        let qualifier = parts[2].split('/').next().unwrap_or(parts[2]);

        let (ark_type, id) = if let Some(id) = qualifier.strip_prefix("an_ua") {
            (ArkType::Act, id.to_string())
        } else if let Some(id) = qualifier.strip_prefix("an_ud") {
            (ArkType::Register, id.to_string())
        } else {
            return Err(RustenatiError::InvalidArk(input.to_string()));
        };

        if id.is_empty() {
            return Err(RustenatiError::InvalidArk(input.to_string()));
        }

        Ok(Self { naan, ark_type, id })
    }

    /// Construct the gallery page URL.
    pub fn gallery_url(&self) -> String {
        let type_prefix = match self.ark_type {
            ArkType::Act => "an_ua",
            ArkType::Register => "an_ud",
        };
        format!(
            "https://antenati.cultura.gov.it/ark:/{}/{}{}",
            self.naan, type_prefix, self.id
        )
    }
}

impl fmt::Display for ArkIdentifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let type_prefix = match self.ark_type {
            ArkType::Act => "an_ua",
            ArkType::Register => "an_ud",
        };
        write!(f, "ark:/{}/{}{}", self.naan, type_prefix, self.id)
    }
}

impl fmt::Display for ArkType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ArkType::Act => write!(f, "Act"),
            ArkType::Register => write!(f, "Register"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_act_ark() {
        let ark = ArkIdentifier::parse("ark:/12657/an_ua12345").unwrap();
        assert_eq!(ark.naan, "12657");
        assert_eq!(ark.ark_type, ArkType::Act);
        assert_eq!(ark.id, "12345");
    }

    #[test]
    fn parse_register_ark() {
        let ark = ArkIdentifier::parse("ark:/12657/an_ud67890").unwrap();
        assert_eq!(ark.naan, "12657");
        assert_eq!(ark.ark_type, ArkType::Register);
        assert_eq!(ark.id, "67890");
    }

    #[test]
    fn parse_full_url() {
        let ark = ArkIdentifier::parse(
            "https://antenati.cultura.gov.it/ark:/12657/an_ua12345",
        )
        .unwrap();
        assert_eq!(ark.ark_type, ArkType::Act);
        assert_eq!(ark.id, "12345");
    }

    #[test]
    fn parse_url_with_canvas_suffix() {
        let ark = ArkIdentifier::parse(
            "https://antenati.cultura.gov.it/ark:/12657/an_ud67890/00001",
        )
        .unwrap();
        assert_eq!(ark.ark_type, ArkType::Register);
        assert_eq!(ark.id, "67890");
    }

    #[test]
    fn invalid_ark() {
        assert!(ArkIdentifier::parse("not-an-ark").is_err());
        assert!(ArkIdentifier::parse("ark:/12657/an_xx123").is_err());
        assert!(ArkIdentifier::parse("ark:/12657/an_ua").is_err());
    }

    #[test]
    fn display_roundtrip() {
        let ark = ArkIdentifier::parse("ark:/12657/an_ua12345").unwrap();
        assert_eq!(ark.to_string(), "ark:/12657/an_ua12345");
    }

    #[test]
    fn gallery_url() {
        let ark = ArkIdentifier::parse("ark:/12657/an_ud67890").unwrap();
        assert_eq!(
            ark.gallery_url(),
            "https://antenati.cultura.gov.it/ark:/12657/an_ud67890"
        );
    }
}
