//! OS decimal-separator detection for the "Copy" (Excel) format.

/// Language codes whose usual decimal separator is a comma (CLDR).
const COMMA_LANGUAGES: &[&str] = &[
    "es", "pt", "fr", "de", "it", "nl", "ru", "pl", "tr", "sv", "da", "nb", "no", "nn", "fi", "cs",
    "id", "ro", "hu", "el", "uk", "bg", "ca", "hr", "sk", "sl", "sr", "lt", "lv", "et", "vi", "az",
    "kk", "be", "ka", "hy", "mk", "bs", "sq", "eu", "gl", "is", "af", "uz", "ky",
];

/// `(language, region, separator)` where a region differs from its language.
const REGION_OVERRIDES: &[(&str, &str, char)] = &[
    // Spanish in the Americas that uses the point (es-419 and its members).
    ("es", "419", '.'),
    ("es", "MX", '.'),
    ("es", "US", '.'),
    ("es", "PE", '.'),
    ("es", "PR", '.'),
    ("es", "DO", '.'),
    ("es", "GT", '.'),
    ("es", "HN", '.'),
    ("es", "NI", '.'),
    ("es", "PA", '.'),
    ("es", "SV", '.'),
    // Swiss and Liechtenstein German/Italian use the point (fr-CH a comma).
    ("de", "CH", '.'),
    ("de", "LI", '.'),
    ("it", "CH", '.'),
    // English in South Africa and continental Europe uses the comma.
    ("en", "ZA", ','),
    ("en", "150", ','),
    ("en", "AT", ','),
    ("en", "BE", ','),
    ("en", "DE", ','),
    ("en", "DK", ','),
    ("en", "FI", ','),
    ("en", "NL", ','),
    ("en", "SE", ','),
    ("en", "SI", ','),
];

/// The decimal separator for a locale tag such as `es-AR`, `pt_BR`,
/// `es-Latn-MX` or `en-US.UTF-8`: the language's, unless its region is in
/// [`REGION_OVERRIDES`].
pub fn decimal_separator_for(locale: &str) -> char {
    let mut parts = locale.split(['-', '_', '.', '@']);
    let lang = parts.next().unwrap_or_default().to_ascii_lowercase();
    let region = parts
        .find(|p| {
            (p.len() == 2 && p.bytes().all(|b| b.is_ascii_alphabetic()))
                || (p.len() == 3 && p.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(str::to_ascii_uppercase);
    if let Some(region) = region {
        if let Some(&(_, _, sep)) = REGION_OVERRIDES
            .iter()
            .find(|(l, r, _)| *l == lang && *r == region)
        {
            return sep;
        }
    }
    if COMMA_LANGUAGES.contains(&lang.as_str()) {
        ','
    } else {
        '.'
    }
}

/// The decimal separator of the operating system's locale, `.` if unknown.
pub fn system_decimal_separator() -> char {
    sys_locale::get_locale()
        .map(|l| decimal_separator_for(&l))
        .unwrap_or('.')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_map() {
        assert_eq!(decimal_separator_for("es-AR"), ',');
        assert_eq!(decimal_separator_for("en-US"), '.');
        assert_eq!(decimal_separator_for("pt_BR"), ',');
        assert_eq!(decimal_separator_for("no-NO"), ',');
        assert_eq!(decimal_separator_for("nn_NO"), ',');
    }

    #[test]
    fn more_comma_languages() {
        for l in [
            "ro-RO", "hu", "el-GR", "uk-UA", "bg", "ca-ES", "hr", "sk", "sl", "sr-RS", "lt", "lv",
            "et", "vi-VN", "az", "kk",
        ] {
            assert_eq!(decimal_separator_for(l), ',', "{l}");
        }
        for l in ["ja-JP", "zh-CN", "ko", "he-IL", "th", "hi-IN", "en-GB"] {
            assert_eq!(decimal_separator_for(l), '.', "{l}");
        }
    }

    #[test]
    fn region_overrides() {
        for l in [
            "es-MX",
            "es-US",
            "es-PE",
            "es-PR",
            "es-DO",
            "es-GT",
            "es-HN",
            "es-NI",
            "es-PA",
            "es-SV",
            "es-419",
            "es_MX.UTF-8",
            "es-Latn-MX",
            "de-CH",
            "de-LI",
            "it-CH",
        ] {
            assert_eq!(decimal_separator_for(l), '.', "{l}");
        }
        for l in [
            "es-AR", "es-ES", "es-UY", "es-CL", "es-CO", "es-VE", "fr-CH", "fr-CA", "de-DE",
            "it-IT", "en-ZA", "en-DE", "es",
        ] {
            assert_eq!(decimal_separator_for(l), ',', "{l}");
        }
    }
}
