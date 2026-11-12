//! Languages of odkvmctl and opendyn-kvm (software.md §9 "Languages"): every text a person reads comes from
//! `i18n/<language>/odkvmctl.ftl` (Project Fluent), embedded in the programs, through [`fl!`](crate::fl).
//!
//! - **Languages:** English (the fallback, and the file `fl!` checks message ids and arguments against at compile
//!   time), French, German, Norwegian Bokmål, Simplified Chinese (`zh`: any Chinese system language gets it). A
//!   message missing from a translation falls back to English.
//! - **Choice:** the system's languages, or the one set in the window (Settings → This computer, saved in
//!   `local::Local::language`), which the command line uses too.
//! - **Branding:** product and feature names are Fluent *terms* at the top of each file (`-product`, `-wireless`,
//!   ...). Messages use them, so a name changes in one line per language, with the grammar each language needs.

use std::sync::LazyLock;

use i18n_embed::fluent::{FluentLanguageLoader, fluent_language_loader};
use i18n_embed::unic_langid::LanguageIdentifier;
use i18n_embed::{DesktopLanguageRequester, LanguageLoader};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "i18n/"]
struct Localizations;

/// The loaded language; [`fl!`](crate::fl) reads it.
pub static LANGUAGE_LOADER: LazyLock<FluentLanguageLoader> = LazyLock::new(|| {
    let loader: FluentLanguageLoader = fluent_language_loader!();
    loader.load_fallback_language(&Localizations).expect("the English messages are embedded");
    // Fluent wraps arguments in Unicode isolation marks for right-to-left text; egui and terminals would show them.
    loader.set_use_isolating(false);
    loader
});

/// The languages there are: code, and the language's own name.
pub const LANGUAGES: &[(&str, &str)] =
    &[("en", "English"), ("fr", "Français"), ("de", "Deutsch"), ("nb", "Norsk bokmål"), ("zh", "简体中文")];

/// A message by id, with its arguments: `fl!("ports-title")`, `fl!("switched-to", pc = name)`.
#[macro_export]
macro_rules! fl {
    ($message_id:literal) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id)
    }};
    ($message_id:literal, $($args:expr),* $(,)?) => {{
        i18n_embed_fl::fl!($crate::i18n::LANGUAGE_LOADER, $message_id, $($args),*)
    }};
}

/// Loads `choice` (a code from [`LANGUAGES`]), or the system's languages when None or unknown; English fills gaps.
pub fn init(choice: Option<&str>) {
    let chosen: Option<LanguageIdentifier> =
        choice.filter(|c| LANGUAGES.iter().any(|(code, _)| code == c)).and_then(|c| c.parse().ok());
    let requested = match chosen {
        Some(l) => vec![l],
        None => with_bokmal(DesktopLanguageRequester::requested_languages()),
    };
    if let Err(e) = i18n_embed::select(&*LANGUAGE_LOADER, &Localizations, &requested) {
        eprintln!("languages: {e}");
    }
    // Bundles loaded now start with isolation on again.
    LANGUAGE_LOADER.set_use_isolating(false);
}

/// `v` with `digits` decimals, written the way the language in use writes them ("5.05" in English and Chinese,
/// "5,05" in French, German and Norwegian). Fluent's own NUMBER doesn't localize the separator.
pub fn decimal(v: f64, digits: usize) -> String {
    let s = format!("{v:.digits$}");
    if matches!(current().as_str(), "en" | "zh") { s } else { s.replace('.', ",") }
}

/// Norwegian systems may ask for `no` (Norwegian) or `nn` (Nynorsk): Bokmål is the translation there is, so it goes
/// right after each.
fn with_bokmal(requested: Vec<LanguageIdentifier>) -> Vec<LanguageIdentifier> {
    let nb: LanguageIdentifier = "nb".parse().expect("a valid language code");
    let mut out = Vec::with_capacity(requested.len() + 1);
    for l in requested {
        let norwegian = matches!(l.language.as_str(), "no" | "nn");
        out.push(l);
        if norwegian && !out.contains(&nb) {
            out.push(nb.clone());
        }
    }
    out
}

/// The language now in use (a code from [`LANGUAGES`]).
pub fn current() -> String {
    LANGUAGE_LOADER.current_language().language.as_str().to_string()
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use fluent_syntax::ast;

    use super::*;

    /// Message and term ids, each with the variables its value and attributes use.
    fn ids(lang: &str) -> BTreeMap<String, BTreeSet<String>> {
        let file = Localizations::get(&format!("{lang}/odkvmctl.ftl")).unwrap_or_else(|| panic!("no {lang} file"));
        let text = std::str::from_utf8(&file.data).unwrap().to_string();
        let res = fluent_syntax::parser::parse(text).unwrap_or_else(|(_, e)| panic!("{lang}: {e:?}"));
        fn vars(p: &ast::Pattern<String>, out: &mut BTreeSet<String>) {
            for el in &p.elements {
                if let ast::PatternElement::Placeable { expression } = el {
                    expr(expression, out);
                }
            }
        }
        fn inline(e: &ast::InlineExpression<String>, out: &mut BTreeSet<String>) {
            match e {
                ast::InlineExpression::VariableReference { id } => {
                    out.insert(id.name.clone());
                }
                ast::InlineExpression::Placeable { expression } => expr(expression, out),
                _ => {}
            }
        }
        fn expr(e: &ast::Expression<String>, out: &mut BTreeSet<String>) {
            match e {
                ast::Expression::Select { selector, variants } => {
                    inline(selector, out);
                    for v in variants {
                        vars(&v.value, out);
                    }
                }
                ast::Expression::Inline(i) => inline(i, out),
            }
        }
        let mut out = BTreeMap::new();
        for entry in res.body {
            if let ast::Entry::Message(m) = entry {
                let mut v = BTreeSet::new();
                if let Some(p) = &m.value {
                    vars(p, &mut v);
                }
                for a in &m.attributes {
                    vars(&a.value, &mut v);
                }
                out.insert(m.id.name.clone(), v);
            }
        }
        out
    }

    #[test]
    fn every_language_has_every_message_with_the_same_arguments() {
        let en = ids("en");
        assert!(en.len() > 100, "{} English messages", en.len());
        for (lang, _) in LANGUAGES.iter().filter(|(l, _)| *l != "en") {
            let other = ids(lang);
            let missing: Vec<_> = en.keys().filter(|k| !other.contains_key(*k)).collect();
            assert!(missing.is_empty(), "{lang} lacks {missing:?}");
            let extra: Vec<_> = other.keys().filter(|k| !en.contains_key(*k)).collect();
            assert!(extra.is_empty(), "{lang} has messages English hasn't: {extra:?}");
            for (id, vars) in &en {
                assert_eq!(&other[id], vars, "{lang} {id}: other arguments than English");
            }
        }
    }

    #[test]
    fn every_message_formats_without_errors_in_every_language() {
        use fluent_bundle::{FluentArgs, FluentBundle, FluentResource, FluentValue};
        for (lang, _) in LANGUAGES {
            let file = Localizations::get(&format!("{lang}/odkvmctl.ftl")).unwrap();
            let text = std::str::from_utf8(&file.data).unwrap().to_string();
            let id: LanguageIdentifier = lang.parse().unwrap();
            let mut bundle = FluentBundle::new(vec![id]);
            bundle.set_use_isolating(false);
            bundle.add_resource(FluentResource::try_new(text).unwrap()).unwrap();
            for (msg, vars) in ids(lang) {
                let mut args = FluentArgs::new();
                for v in &vars {
                    args.set(v.clone(), FluentValue::from(2));
                }
                let pattern = bundle.get_message(&msg).and_then(|m| m.value()).unwrap();
                let mut errors = Vec::new();
                let out = bundle.format_pattern(pattern, Some(&args), &mut errors);
                assert!(errors.is_empty(), "{lang} {msg}: {errors:?}");
                assert!(!out.trim().is_empty(), "{lang} {msg} is empty");
            }
        }
    }

    #[test]
    fn each_language_loads_and_falls_back_to_english() {
        // A loader of its own: the shared one is the programs' (and other tests read English from it).
        for (lang, _) in LANGUAGES {
            let loader: FluentLanguageLoader = fluent_language_loader!();
            loader.set_use_isolating(false);
            let id: LanguageIdentifier = lang.parse().unwrap();
            loader.load_languages(&Localizations, std::slice::from_ref(&id)).unwrap();
            assert_eq!(loader.current_language(), id);
            assert_eq!(loader.get("product-name"), "OpenDynKVM", "{lang}");
        }
        assert_eq!(crate::fl!("product-name"), "OpenDynKVM");
    }

    #[test]
    fn a_chinese_system_gets_the_chinese_translation() {
        // The system asks with a region or a script; the translation is `zh` (Simplified).
        for system in ["zh-CN", "zh-Hans-CN", "zh-SG", "zh"] {
            let loader: FluentLanguageLoader = fluent_language_loader!();
            loader.load_fallback_language(&Localizations).unwrap();
            let requested: Vec<LanguageIdentifier> = vec![system.parse().unwrap()];
            i18n_embed::select(&loader, &Localizations, &requested).unwrap();
            assert_eq!(loader.current_language().language.as_str(), "zh", "{system}");
        }
    }
}
