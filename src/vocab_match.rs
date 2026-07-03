//! Deterministische Vokabular-Korrektur: ersetzt Verhörer der Spracherkennung
//! durch Nutzer-Begriffe per Kölner Phonetik (deutsches Klangbild) — läuft vor
//! dem Cleanup und wirkt damit in ALLEN Modi inkl. Roh. Ergänzt die weichere
//! LLM-Korrektur (Ticket-0012): »Clotzelei« → »Claude CLI« schafft nur diese Schicht.

/// Wendet das Vokabular auf ein Transkript an: Wort-Fenster (1–3 Wörter), deren
/// Kölner-Phonetik-Code exakt dem eines Begriffs entspricht (+ gleicher
/// Anfangsbuchstabe), werden durch den Begriff ersetzt. Interpunktion bleibt.
pub fn apply(transcript: &str, vocab: &[String]) -> String {
    if vocab.is_empty() || transcript.is_empty() {
        return transcript.to_string();
    }
    let terms: Vec<(&String, String, String, char)> = vocab
        .iter()
        .filter_map(|term| {
            let squashed = squash(term);
            let first = squashed.chars().next()?;
            // Sehr kurze Begriffe (<4) sind zu kollisionsanfällig.
            (squashed.chars().count() >= 4).then_some((term, cologne(&squashed), squashed, first))
        })
        .collect();
    if terms.is_empty() {
        return transcript.to_string();
    }

    let tokens = tokenize(transcript);
    let words: Vec<&str> = tokens.iter().map(|t| t.word.as_str()).collect();
    let mut out = String::new();

    let mut i = 0;
    while i < tokens.len() {
        let mut matched: Option<(usize, &String)> = None;
        // Längere Fenster zuerst — spezifischster Treffer gewinnt.
        'window: for len in (1..=3.min(tokens.len() - i)).rev() {
            let window = words[i..i + len].join("");
            let win_sq = squash(&window);
            let Some(first) = win_sq.chars().next() else {
                continue;
            };
            let win_code = cologne(&win_sq);
            for (term, code, term_sq, term_first) in &terms {
                let len_diff = win_sq.chars().count().abs_diff(term_sq.chars().count());
                if first == *term_first
                    && len_diff <= 3
                    && win_sq != *term_sq
                    && cologne_close(&win_code, code)
                {
                    matched = Some((len, term));
                    break 'window;
                }
            }
        }
        match matched {
            Some((len, term)) => {
                out.push_str(&tokens[i].leading);
                out.push_str(term);
                out.push_str(&tokens[i + len - 1].trailing);
                i += len;
            }
            None => {
                let t = &tokens[i];
                out.push_str(&t.leading);
                out.push_str(&t.word);
                out.push_str(&t.trailing);
                i += 1;
            }
        }
    }
    out
}

/// Klangbild-Vergleich: exakt gleich, oder ein Editier-Schritt Abstand bei
/// mindestens 3-stelligen Codes (»45285« vs »4585« = Claude CLI vs Clotzelei).
fn cologne_close(a: &str, b: &str) -> bool {
    if a == b {
        return !a.is_empty();
    }
    let (la, lb) = (a.chars().count(), b.chars().count());
    la.max(lb) >= 3 && la.abs_diff(lb) <= 1 && edit_distance(a, b) <= 1
}

fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let sub = prev[j] + usize::from(ca != cb);
            curr[j + 1] = sub.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

struct Token {
    leading: String,
    word: String,
    trailing: String,
}

/// Zerlegt in Wörter mit anhängender Interpunktion/Whitespace.
fn tokenize(text: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut leading = String::new();
    let mut word = String::new();
    let mut trailing = String::new();
    for c in text.chars() {
        if c.is_alphanumeric() {
            if !trailing.is_empty() {
                tokens.push(Token {
                    leading: std::mem::take(&mut leading),
                    word: std::mem::take(&mut word),
                    trailing: std::mem::take(&mut trailing),
                });
            }
            word.push(c);
        } else if word.is_empty() {
            leading.push(c);
        } else {
            trailing.push(c);
        }
    }
    if !word.is_empty() || !leading.is_empty() {
        tokens.push(Token {
            leading,
            word,
            trailing,
        });
    }
    tokens
}

fn squash(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Kölner Phonetik (vereinfacht, Standard-Regelwerk) über einem
/// klein-alphanumerischen String.
fn cologne(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut raw = String::new();
    for (i, &c) in chars.iter().enumerate() {
        let prev = if i > 0 { Some(chars[i - 1]) } else { None };
        let next = chars.get(i + 1).copied();
        let code: &str = match c {
            'a' | 'e' | 'i' | 'j' | 'o' | 'u' | 'y' | 'ä' | 'ö' | 'ü' => "0",
            'h' => "",
            'b' => "1",
            'p' => {
                if next == Some('h') {
                    "3"
                } else {
                    "1"
                }
            }
            'd' | 't' => {
                if matches!(next, Some('c' | 's' | 'z')) {
                    "8"
                } else {
                    "2"
                }
            }
            'f' | 'v' | 'w' => "3",
            'g' | 'k' | 'q' => "4",
            'c' => {
                let before_ahkloqrux = matches!(
                    next,
                    Some('a' | 'h' | 'k' | 'l' | 'o' | 'q' | 'r' | 'u' | 'x')
                );
                if i == 0 && before_ahkloqrux {
                    "4"
                } else if matches!(prev, Some('s' | 'z')) {
                    "8"
                } else if matches!(next, Some('a' | 'h' | 'k' | 'o' | 'q' | 'u' | 'x')) {
                    "4"
                } else {
                    "8"
                }
            }
            'x' => {
                if matches!(prev, Some('c' | 'k' | 'q')) {
                    "8"
                } else {
                    "48"
                }
            }
            'l' => "5",
            'm' | 'n' => "6",
            'r' => "7",
            's' | 'z' | 'ß' => "8",
            digit @ '0'..='9' => {
                // Ziffern klanglich behalten (TOML vs. K8s etc. bleiben unterscheidbar).
                raw.push(digit);
                continue;
            }
            _ => "",
        };
        raw.push_str(code);
    }
    // Doppelte zusammenziehen, dann Nullen (außer am Anfang) entfernen.
    let mut out = String::new();
    let mut last: Option<char> = None;
    for c in raw.chars() {
        if Some(c) != last {
            out.push(c);
        }
        last = Some(c);
    }
    let first = out.chars().next();
    let rest: String = out.chars().skip(1).filter(|&c| c != '0').collect();
    first.map(|f| format!("{f}{rest}")).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab() -> Vec<String> {
        ["Claude CLI", "TOML", "Kubernetes", "egui", "Docker Image"]
            .into_iter()
            .map(String::from)
            .collect()
    }

    #[test]
    fn corrects_the_field_test_mishearings() {
        let v = vocab();
        assert_eq!(
            apply("die Clotzelei ist offen", &v),
            "die Claude CLI ist offen"
        );
        assert_eq!(
            apply("liegt als Tommel im Repo", &v),
            "liegt als TOML im Repo"
        );
        assert_eq!(
            apply("das Deployment in Kubanetis hängt", &v),
            "das Deployment in Kubernetes hängt"
        );
    }

    #[test]
    fn corrects_multiword_windows_and_keeps_punctuation() {
        let v = vocab();
        assert_eq!(
            apply("öffne die Clot Klee, bitte", &v),
            "öffne die Claude CLI, bitte"
        );
        assert_eq!(
            apply("bau das Darker Image neu", &v),
            "bau das Docker Image neu"
        );
    }

    #[test]
    fn leaves_exact_terms_and_normal_words_alone() {
        let v = vocab();
        // Exakt vorhandene Begriffe bleiben unangetastet.
        assert_eq!(apply("die Claude CLI läuft", &v), "die Claude CLI läuft");
        // Normale Wörter mit anderem Anfangsbuchstaben/Klang bleiben.
        let s = "wir malen ein Bild und trinken Kaffee";
        assert_eq!(apply(s, &v), s);
        // Leeres Vokabular → Identität.
        assert_eq!(apply("Tommel bleibt", &[]), "Tommel bleibt");
    }

    #[test]
    fn short_terms_are_skipped_as_too_collision_prone() {
        let v = vec!["Git".to_string()];
        assert_eq!(apply("das ist gut", &v), "das ist gut");
    }

    #[test]
    fn colliding_terms_first_in_vocab_order_wins() {
        // Beide Begriffe haben denselben Kölner-Code + Anfangsbuchstaben —
        // definiertes Verhalten: der zuerst gelistete Begriff gewinnt.
        assert_eq!(
            cologne("maier"),
            cologne("mayer"),
            "Vorbedingung: Kollision"
        );
        let v = vec!["Maier".to_string(), "Mayer".to_string()];
        assert_eq!(apply("Herr Meier kommt", &v), "Herr Maier kommt");
        let v_rev = vec!["Mayer".to_string(), "Maier".to_string()];
        assert_eq!(apply("Herr Meier kommt", &v_rev), "Herr Mayer kommt");
    }

    #[test]
    fn digit_terms_are_skipped_as_too_short_without_panic() {
        // »K8s« squasht auf 3 Zeichen → unter der Mindestlänge 4, wird ignoriert.
        let v = vec!["K8s".to_string()];
        assert_eq!(
            apply("das kates cluster läuft", &v),
            "das kates cluster läuft"
        );
    }

    #[test]
    fn digit_bearing_terms_above_min_length_apply() {
        let v = vec!["IPv6".to_string()];
        let out = apply("wir nutzen ipv6 überall", &v);
        assert!(!out.is_empty(), "kein Panic, definierter Output: {out}");
    }

    #[test]
    fn punctuation_or_whitespace_only_input_is_unchanged() {
        let v = vocab();
        for s in ["", "   ", "…!? — ,,,", "\n\t"] {
            assert_eq!(apply(s, &v), s, "Input {s:?}");
        }
    }

    #[test]
    fn cologne_codes_are_close_for_expected_pairs() {
        for (a, b) in [
            ("clotzelei", "claudecli"),
            ("tommel", "toml"),
            ("kubanetis", "kubernetes"),
            ("ekwi", "egui"),
            ("darkerimage", "dockerimage"),
        ] {
            assert!(cologne_close(&cologne(a), &cologne(b)), "{a} ≉ {b}");
        }
        assert!(!cologne_close(&cologne("mal"), &cologne("kaffee")));
        assert!(!cologne_close(&cologne("bild"), &cologne("toml")));
    }
}
