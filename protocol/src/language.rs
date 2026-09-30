//! Language identifiers.

use serde::{Deserialize, Serialize};
use std::fmt;

/// A language the bridge can translate between.
///
/// BCP-47 primary subtags, lowercased. Kept as an enum rather than a bare
/// `String` so the client cannot request an unconfigured pair and the gateway
/// can price and route without parsing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// Thai. Primary launch source language.
    Thai,
    /// English. Primary launch target language.
    English,
    /// Japanese.
    Japanese,
    /// Korean.
    Korean,
    /// Simplified Chinese.
    Chinese,
    /// Vietnamese.
    Vietnamese,
    /// Indonesian.
    Indonesian,
    /// Spanish.
    Spanish,
    /// Portuguese (Brazil).
    Portuguese,
    /// French.
    French,
    /// German.
    German,
    /// Russian.
    Russian,
    /// Turkish.
    Turkish,
    /// Arabic.
    Arabic,
    /// Hindi.
    Hindi,
}

impl Language {
    /// BCP-47 primary subtag, e.g. `"th"`.
    pub const fn code(self) -> &'static str {
        match self {
            Language::Thai => "th",
            Language::English => "en",
            Language::Japanese => "ja",
            Language::Korean => "ko",
            Language::Chinese => "zh",
            Language::Vietnamese => "vi",
            Language::Indonesian => "id",
            Language::Spanish => "es",
            Language::Portuguese => "pt",
            Language::French => "fr",
            Language::German => "de",
            Language::Russian => "ru",
            Language::Turkish => "tr",
            Language::Arabic => "ar",
            Language::Hindi => "hi",
        }
    }

    /// English display name, for UI labels and logs.
    pub const fn display_name(self) -> &'static str {
        match self {
            Language::Thai => "Thai",
            Language::English => "English",
            Language::Japanese => "Japanese",
            Language::Korean => "Korean",
            Language::Chinese => "Chinese",
            Language::Vietnamese => "Vietnamese",
            Language::Indonesian => "Indonesian",
            Language::Spanish => "Spanish",
            Language::Portuguese => "Portuguese",
            Language::French => "French",
            Language::German => "German",
            Language::Russian => "Russian",
            Language::Turkish => "Turkish",
            Language::Arabic => "Arabic",
            Language::Hindi => "Hindi",
        }
    }

    /// Two-letter uppercase tag shown in the UI hero card, e.g. `"TH"`.
    pub fn short_tag(self) -> String {
        self.code().to_uppercase()
    }

    /// Parse a BCP-47 primary subtag, case-insensitively.
    pub fn from_code(code: &str) -> Option<Self> {
        // Tolerate region subtags like "pt-BR" by looking only at the primary.
        let primary = code.split(['-', '_']).next()?.to_ascii_lowercase();
        Some(match primary.as_str() {
            "th" => Language::Thai,
            "en" => Language::English,
            "ja" => Language::Japanese,
            "ko" => Language::Korean,
            "zh" => Language::Chinese,
            "vi" => Language::Vietnamese,
            "id" => Language::Indonesian,
            "es" => Language::Spanish,
            "pt" => Language::Portuguese,
            "fr" => Language::French,
            "de" => Language::German,
            "ru" => Language::Russian,
            "tr" => Language::Turkish,
            "ar" => Language::Arabic,
            "hi" => Language::Hindi,
            _ => return None,
        })
    }

    /// Whether this language is written right-to-left.
    ///
    /// The subtitle overlay uses this to pick text alignment and to decide
    /// whether mixed-direction lines need bidi isolation.
    pub const fn is_rtl(self) -> bool {
        matches!(self, Language::Arabic)
    }
}

impl fmt::Display for Language {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_roundtrips_for_every_language() {
        for lang in [
            Language::Thai,
            Language::English,
            Language::Japanese,
            Language::Korean,
            Language::Chinese,
            Language::Vietnamese,
            Language::Indonesian,
            Language::Spanish,
            Language::Portuguese,
            Language::French,
            Language::German,
            Language::Russian,
            Language::Turkish,
            Language::Arabic,
            Language::Hindi,
        ] {
            assert_eq!(Language::from_code(lang.code()), Some(lang));
        }
    }

    #[test]
    fn from_code_is_case_insensitive_and_strips_region() {
        assert_eq!(Language::from_code("TH"), Some(Language::Thai));
        assert_eq!(Language::from_code("pt-BR"), Some(Language::Portuguese));
        assert_eq!(Language::from_code("zh_Hans"), Some(Language::Chinese));
    }

    #[test]
    fn from_code_rejects_unknown() {
        assert_eq!(Language::from_code("klingon"), None);
        assert_eq!(Language::from_code(""), None);
    }

    #[test]
    fn short_tag_is_uppercase_primary() {
        assert_eq!(Language::Thai.short_tag(), "TH");
        assert_eq!(Language::English.short_tag(), "EN");
    }

    #[test]
    fn only_arabic_is_rtl() {
        assert!(Language::Arabic.is_rtl());
        assert!(!Language::Thai.is_rtl());
        assert!(!Language::English.is_rtl());
    }

    #[test]
    fn thai_is_the_lowercase_serde_form() {
        let json = serde_json::to_string(&Language::Thai).unwrap();
        assert_eq!(json, "\"thai\"");
    }
}
