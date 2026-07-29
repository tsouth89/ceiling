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
fn test_locale_key_chinese() {
    // Sample keys must resolve to the Simplified Chinese bundle, proving the
    // language switch is wired end to end (settings -> language_id -> Fluent).
    assert_eq!(get_text(Language::Chinese, LocaleKey::TabGeneral), "常规");
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::InterfaceLanguage),
        "界面语言"
    );
    assert_eq!(get_text(Language::Chinese, LocaleKey::MenuQuit), "退出");
}

#[test]
fn test_chinese_preserves_format_placeholders() {
    // The `{}`/`{:.0}` tokens are filled later by `format_template`, so a
    // translation that drops or mangles them would break runtime formatting.
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::UsedPercent),
        "已使用 {:.0}%"
    );
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::TrayResetsInLabel),
        "{} 后重置"
    );
    // Leading-space status overlays stay intact so tray tooltips concatenate.
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::TrayStatusError),
        " (错误)"
    );
}

#[test]
fn test_all_locale_keys_present_in_chinese_with_matching_placeholders() {
    let zh_cn = resource_key_names(include_str!("zh-CN.ftl"));
    for (key, name) in LocaleKey::ALL {
        assert!(zh_cn.contains(name), "missing Fluent key {name} in zh-CN");

        let english = get_text(Language::English, *key);
        let chinese = get_text(Language::Chinese, *key);
        assert!(
            !chinese.trim().is_empty(),
            "missing or empty Chinese Fluent text for {key:?}"
        );
        assert_eq!(
            format_placeholders(&chinese),
            format_placeholders(&english),
            "format placeholders differ for {name}"
        );
    }

    assert_eq!(
        format_template(
            &get_text(Language::Chinese, LocaleKey::TrayResetsInLabel),
            &["5 分钟"],
        ),
        "5 分钟 后重置"
    );
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::UsedPercent).replace("{:.0}", "42"),
        "已使用 42%"
    );
}

#[test]
fn test_untranslated_language_falls_back_to_english() {
    // Languages without their own bundle (e.g. Spanish) resolve to English
    // rather than leaking a raw key name.
    assert_eq!(
        get_text(Language::Spanish, LocaleKey::TabGeneral),
        "General"
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

fn format_placeholders(text: &str) -> Vec<&str> {
    let mut placeholders = Vec::new();
    let mut remainder = text;
    while let Some(start) = remainder.find('{') {
        let candidate = &remainder[start..];
        let Some(end) = candidate.find('}') else {
            break;
        };
        placeholders.push(&candidate[..=end]);
        remainder = &candidate[end + 1..];
    }
    placeholders.sort_unstable();
    placeholders
}
