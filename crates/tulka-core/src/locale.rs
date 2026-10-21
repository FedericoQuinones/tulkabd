//! OS decimal-separator detection for the "Copy" (Excel) format.

/// Language codes whose usual decimal separator is a comma.
const COMMA_LANGUAGES: [&str; 15] = [
    "es", "pt", "fr", "de", "it", "nl", "ru", "pl", "tr", "sv", "da", "nb", "fi", "cs", "id",
];

/// The decimal separator for a locale tag such as `es-AR`, `pt_BR` or `en-US`.
/// Only the language prefix is used.
pub fn decimal_separator_for(locale: &str) -> char {
    let lang = locale
        .split(['-', '_', '.', '@'])
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
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
    }
}
