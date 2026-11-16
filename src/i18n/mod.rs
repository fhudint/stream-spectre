//! UI text catalogs.
//!
//! All text shown to viewers and moderators lives in `locales/<lang>.json`
//! (flat `"key": "text"` objects, embedded in the binary). Log and error
//! messages meant for developers stay in English in the code.
//!
//! Placeholders look like `{name}`. Values are inserted as plain text and are
//! never interpreted, so callers (and the web pages) must still escape them
//! for their output context; the pages do so by using `textContent`.

use std::collections::HashMap;

use crate::config::Language;

const ID_JSON: &str = include_str!("../../locales/id.json");
const EN_JSON: &str = include_str!("../../locales/en.json");

/// Raw JSON catalog for a language, for serving to the web pages.
pub fn catalog_json(language: Language) -> &'static str {
    match language {
        Language::Id => ID_JSON,
        Language::En => EN_JSON,
    }
}

#[derive(Debug, thiserror::Error)]
#[error("cannot parse the {language:?} locale file: {source}")]
pub struct I18nError {
    language: Language,
    #[source]
    source: serde_json::Error,
}

/// A loaded catalog for one language.
#[derive(Debug, Clone)]
pub struct I18n {
    language: Language,
    messages: HashMap<String, String>,
}

impl I18n {
    pub fn load(language: Language) -> Result<Self, I18nError> {
        let messages = serde_json::from_str(catalog_json(language))
            .map_err(|source| I18nError { language, source })?;
        Ok(Self { language, messages })
    }

    pub fn language(&self) -> Language {
        self.language
    }

    /// Text for `key`. A missing key yields the key itself, so a gap is
    /// visible in the UI instead of showing nothing.
    pub fn t<'a>(&'a self, key: &'a str) -> &'a str {
        self.messages.get(key).map_or(key, String::as_str)
    }

    /// Like `t`, with `{name}` placeholders replaced from `args`.
    pub fn t_with(&self, key: &str, args: &[(&str, &str)]) -> String {
        format_template(self.t(key), args)
    }
}

/// Replaces `{name}` placeholders in one pass. Unknown placeholders and stray
/// braces are kept as written, and inserted values are never re-scanned.
fn format_template(template: &str, args: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;

    while let Some(open) = rest.find('{') {
        let (head, tail) = rest.split_at(open);
        out.push_str(head);
        let after_brace = tail.get(1..).unwrap_or_default();

        match after_brace.find('}') {
            Some(close) => {
                let (name, remainder) = after_brace.split_at(close);
                if name.contains('{') {
                    // This '{' is literal text; retry from the inner one.
                    out.push('{');
                    rest = after_brace;
                    continue;
                }
                match args.iter().find(|(key, _)| *key == name) {
                    Some((_, value)) => out.push_str(value),
                    None => {
                        out.push('{');
                        out.push_str(name);
                        out.push('}');
                    }
                }
                rest = remainder.get(1..).unwrap_or_default();
            }
            None => {
                out.push('{');
                rest = after_brace;
            }
        }
    }

    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    const LANGUAGES: [Language; 2] = [Language::Id, Language::En];

    fn catalog(language: Language) -> HashMap<String, String> {
        serde_json::from_str(catalog_json(language)).unwrap()
    }

    /// Placeholder names in a template; panics on malformed placeholders.
    fn placeholders(template: &str) -> BTreeSet<String> {
        let mut names = BTreeSet::new();
        let mut rest = template;
        while let Some(open) = rest.find(['{', '}']) {
            let (_, tail) = rest.split_at(open);
            assert!(tail.starts_with('{'), "stray '}}' in {template:?}");
            let after = &tail[1..];
            let close = after
                .find('}')
                .unwrap_or_else(|| panic!("unclosed '{{' in {template:?}"));
            let name = &after[..close];
            assert!(
                !name.is_empty() && name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "bad placeholder {name:?} in {template:?}"
            );
            names.insert(name.to_owned());
            rest = &after[close + 1..];
        }
        names
    }

    #[test]
    fn every_locale_parses() {
        for language in LANGUAGES {
            I18n::load(language).unwrap();
        }
    }

    #[test]
    fn locales_have_identical_keys() {
        let id: BTreeSet<_> = catalog(Language::Id).into_keys().collect();
        let en: BTreeSet<_> = catalog(Language::En).into_keys().collect();
        let only_id: Vec<_> = id.difference(&en).collect();
        let only_en: Vec<_> = en.difference(&id).collect();
        assert!(only_id.is_empty(), "only in id.json: {only_id:?}");
        assert!(only_en.is_empty(), "only in en.json: {only_en:?}");
    }

    #[test]
    fn no_value_is_blank() {
        for language in LANGUAGES {
            for (key, value) in catalog(language) {
                assert!(!value.trim().is_empty(), "{language:?}: {key} is blank");
            }
        }
    }

    #[test]
    fn placeholders_are_well_formed_and_match_across_languages() {
        let id = catalog(Language::Id);
        let en = catalog(Language::En);
        for (key, id_text) in &id {
            let en_text = &en[key];
            assert_eq!(
                placeholders(id_text),
                placeholders(en_text),
                "placeholder mismatch for {key}"
            );
        }
    }

    #[test]
    fn keys_are_dotted_lowercase() {
        for key in catalog(Language::En).keys() {
            assert!(
                key.split('.').all(|part| !part.is_empty()
                    && part.chars().all(|c| c.is_ascii_lowercase() || c == '_')),
                "bad key {key:?}"
            );
        }
    }

    #[test]
    fn t_returns_text_and_falls_back_to_key() {
        let i18n = I18n::load(Language::En).unwrap();
        assert_eq!(i18n.language(), Language::En);
        assert_eq!(i18n.t("dashboard.action.approve"), "Approve");
        assert_eq!(i18n.t("no.such.key"), "no.such.key");

        let id = I18n::load(Language::Id).unwrap();
        assert_eq!(id.t("dashboard.action.approve"), "Setujui");
    }

    #[test]
    fn t_with_fills_placeholders() {
        let i18n = I18n::load(Language::En).unwrap();
        assert_eq!(
            i18n.t_with("reject.cooldown", &[("seconds", "30")]),
            "Wait 30 seconds before submitting again"
        );
        assert_eq!(
            I18n::load(Language::Id)
                .unwrap()
                .t_with("overlay.requested_by", &[("name", "Budi")]),
            "Diminta oleh Budi"
        );
    }

    #[test]
    fn inserted_values_are_not_expanded_or_interpreted() {
        let out = format_template(
            "hi {name} and {other}",
            &[("name", "{other}"), ("other", "X")],
        );
        assert_eq!(out, "hi {other} and X");

        let html = format_template("by {name}", &[("name", "<script>alert(1)</script>")]);
        assert_eq!(html, "by <script>alert(1)</script>");
    }

    #[test]
    fn unknown_placeholders_and_stray_braces_are_kept() {
        assert_eq!(format_template("a {x} b", &[]), "a {x} b");
        assert_eq!(format_template("open { brace", &[]), "open { brace");
        assert_eq!(format_template("close } only", &[]), "close } only");
        assert_eq!(format_template("{{name}}", &[("name", "v")]), "{v}");
        assert_eq!(format_template("", &[("a", "b")]), "");
        assert_eq!(format_template("{", &[]), "{");
        assert_eq!(format_template("{}", &[]), "{}");
    }

    #[test]
    fn multibyte_text_is_preserved() {
        assert_eq!(format_template("日本語 {n} …", &[("n", "é")]), "日本語 é …");
    }

    #[test]
    fn catalog_json_is_the_embedded_file() {
        assert!(catalog_json(Language::Id).contains("Dasbor Moderasi"));
        assert!(catalog_json(Language::En).contains("Moderation Dashboard"));
    }
}
