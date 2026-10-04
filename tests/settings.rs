use linknote::settings::{Settings, Store, Theme};
use std::fs;

#[test]
fn preferences_preserve_unknown_fields_and_refuse_external_changes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, r#"{"theme":"light","future_setting":{"a":1}}"#).unwrap();
    let (mut store, mut settings) = Store::load(path.clone());
    assert!(store.warning.is_none());
    assert_eq!(settings.theme, Theme::Light);
    settings.theme = Theme::Dark;
    store.save(&settings).unwrap();
    let saved: serde_json::Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved["future_setting"]["a"], 1);
    fs::write(&path, r#"{"theme":"light"}"#).unwrap();
    assert!(
        store
            .save(&settings)
            .unwrap_err()
            .to_string()
            .contains("其他窗口")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), r#"{"theme":"light"}"#);
}

#[test]
fn corrupt_settings_and_invalid_font_sizes_are_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    for source in [
        "{broken",
        r#"{"mermaid_font_size":65535}"#,
        r#"{"theme":"invalid"}"#,
    ] {
        let path = dir.path().join("settings.json");
        fs::write(&path, source).unwrap();
        let (mut store, settings) = Store::load(path.clone());
        assert_eq!(settings, Settings::default());
        assert!(store.warning.is_some());
        assert!(store.save(&settings).is_err());
        assert_eq!(fs::read_to_string(path).unwrap(), source);
    }
}
