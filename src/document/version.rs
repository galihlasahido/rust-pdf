//! PDF Version handling.

use std::fmt;

/// PDF version identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub enum PdfVersion {
    /// PDF 1.0
    V1_0,
    /// PDF 1.1
    V1_1,
    /// PDF 1.2
    V1_2,
    /// PDF 1.3
    V1_3,
    /// PDF 1.4
    V1_4,
    /// PDF 1.5
    V1_5,
    /// PDF 1.6
    V1_6,
    /// PDF 1.7 (ISO 32000-1:2008)
    #[default]
    V1_7,
    /// PDF 2.0 (ISO 32000-2:2020)
    V2_0,
}

impl PdfVersion {
    /// Returns the version string for the PDF header (e.g., "1.7").
    pub fn as_str(&self) -> &'static str {
        match self {
            PdfVersion::V1_0 => "1.0",
            PdfVersion::V1_1 => "1.1",
            PdfVersion::V1_2 => "1.2",
            PdfVersion::V1_3 => "1.3",
            PdfVersion::V1_4 => "1.4",
            PdfVersion::V1_5 => "1.5",
            PdfVersion::V1_6 => "1.6",
            PdfVersion::V1_7 => "1.7",
            PdfVersion::V2_0 => "2.0",
        }
    }

    /// Returns the major version number.
    pub fn major(&self) -> u8 {
        match self {
            PdfVersion::V2_0 => 2,
            _ => 1,
        }
    }

    /// Returns the minor version number.
    pub fn minor(&self) -> u8 {
        match self {
            PdfVersion::V1_0 => 0,
            PdfVersion::V1_1 => 1,
            PdfVersion::V1_2 => 2,
            PdfVersion::V1_3 => 3,
            PdfVersion::V1_4 => 4,
            PdfVersion::V1_5 => 5,
            PdfVersion::V1_6 => 6,
            PdfVersion::V1_7 => 7,
            PdfVersion::V2_0 => 0,
        }
    }

    /// Checks if a feature is supported in this version.
    ///
    /// Returns true if the feature requires a version <= this version.
    pub fn supports(&self, required: PdfVersion) -> bool {
        *self >= required
    }
}

impl fmt::Display for PdfVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

impl TryFrom<&str> for PdfVersion {
    type Error = String;

    fn try_from(s: &str) -> Result<Self, Self::Error> {
        match s {
            "1.0" => Ok(PdfVersion::V1_0),
            "1.1" => Ok(PdfVersion::V1_1),
            "1.2" => Ok(PdfVersion::V1_2),
            "1.3" => Ok(PdfVersion::V1_3),
            "1.4" => Ok(PdfVersion::V1_4),
            "1.5" => Ok(PdfVersion::V1_5),
            "1.6" => Ok(PdfVersion::V1_6),
            "1.7" => Ok(PdfVersion::V1_7),
            "2.0" => Ok(PdfVersion::V2_0),
            _ => Err(format!("Unknown PDF version: {}", s)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_string() {
        assert_eq!(PdfVersion::V1_7.as_str(), "1.7");
        assert_eq!(PdfVersion::V2_0.as_str(), "2.0");
    }

    #[test]
    fn test_version_ordering() {
        assert!(PdfVersion::V1_7 > PdfVersion::V1_4);
        assert!(PdfVersion::V2_0 > PdfVersion::V1_7);
    }

    #[test]
    fn test_version_supports() {
        assert!(PdfVersion::V1_7.supports(PdfVersion::V1_4));
        assert!(!PdfVersion::V1_4.supports(PdfVersion::V1_7));
    }

    #[test]
    fn test_default() {
        assert_eq!(PdfVersion::default(), PdfVersion::V1_7);
    }

    #[test]
    fn test_try_from() {
        assert_eq!(PdfVersion::try_from("1.7"), Ok(PdfVersion::V1_7));
        assert!(PdfVersion::try_from("3.0").is_err());
    }
}
