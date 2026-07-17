use super::*;
use std::collections::HashSet;

#[test]
fn test_locale_key_english() {
    assert_eq!(
        get_text(Language::English, LocaleKey::TabGeneral),
        "General"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::InterfaceLanguage),
        "Interface Language"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::StartAtLogin),
        "Start at Login"
    );
}

#[test]
fn test_all_locale_keys_present_in_english() {
    let en_us = resource_key_names(include_str!("en-US.ftl"));
    for (key, name) in LocaleKey::ALL {
        assert_eq!(key.name(), *name);
        assert!(en_us.contains(name), "missing Fluent key {name} in en-US");
        let text = get_text(Language::English, *key);
        assert!(
            !text.trim().is_empty(),
            "missing or empty Fluent text for {key:?}"
        );
    }
}

#[test]
fn test_fluent_preserves_literal_placeholders_and_status_spacing() {
    assert_eq!(
        get_text(Language::English, LocaleKey::TrayStatusError),
        " (Error)"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::TrayCreditsRemaining),
        "Credits remaining {}%"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::UsedPercent),
        "{:.0}% used"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::RemainingAmount),
        "{:.2} remaining"
    );
}

fn resource_key_names(resource: &str) -> HashSet<&str> {
    resource
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect()
}
