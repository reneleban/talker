//! Helfer für die Regression „erstes Wort" (REQ-DIAG-032/-033). Liegt unter `tests/support/`,
//! damit der Integrationstest (`tests/stt_nemotron.rs`) und ein Lib-Test (CI, ohne Modelle)
//! denselben Code und dieselben Unit-Tests verwenden.

/// Text → vergleichbare Wörter: Kleinbuchstaben, Umlaute und ß ausgeschrieben, alles
/// Nicht-Alphanumerische trennt (auch Bindestriche), Ziffern bleiben Ziffern.
/// Zahlwörter („drei") werden nicht in Ziffern umgewandelt.
pub fn normalize_words(text: &str) -> Vec<String> {
    let mut spelled = String::new();
    for c in text.to_lowercase().chars() {
        match c {
            'ä' => spelled.push_str("ae"),
            'ö' => spelled.push_str("oe"),
            'ü' => spelled.push_str("ue"),
            'ß' => spelled.push_str("ss"),
            c if c.is_alphanumeric() => spelled.push(c),
            _ => spelled.push(' '),
        }
    }
    spelled.split_whitespace().map(str::to_string).collect()
}

/// Erstes normalisiertes Wort der Referenz; leere oder wortlose Referenz → `None`.
pub fn first_word(reference: &str) -> Option<String> {
    normalize_words(reference).into_iter().next()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn punctuation_and_case_are_ignored() {
        assert_eq!(first_word("Er sagte, dass …").as_deref(), Some("er"));
        assert_eq!(first_word("  »Hallo!« sagte er").as_deref(), Some("hallo"));
        assert_eq!(normalize_words("Hallo, Welt!"), ["hallo", "welt"]);
    }

    #[test]
    fn umlauts_and_sharp_s_are_spelled_out() {
        assert_eq!(first_word("Über die Brücke").as_deref(), Some("ueber"));
        assert_eq!(normalize_words("Größe"), ["groesse"]);
        assert_eq!(normalize_words("Groesse"), ["groesse"]);
        assert_eq!(first_word("Äpfel").as_deref(), Some("aepfel"));
    }

    #[test]
    fn hyphens_split_and_numbers_stay_digits() {
        assert_eq!(normalize_words("WLAN-Türklingel"), ["wlan", "tuerklingel"]);
        assert_eq!(first_word("3. März").as_deref(), Some("3"));
        assert_eq!(normalize_words("12,5 Euro"), ["12", "5", "euro"]);
        // Zahlwörter bleiben Wörter: „drei" ist nicht „3".
        assert_ne!(first_word("drei"), first_word("3"));
    }

    #[test]
    fn empty_or_wordless_reference_has_no_first_word() {
        assert_eq!(first_word(""), None);
        assert_eq!(first_word("   \n"), None);
        assert_eq!(first_word("…!?"), None);
        assert!(normalize_words("").is_empty());
    }
}
