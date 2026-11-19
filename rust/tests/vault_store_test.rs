use tempfile::tempdir;
use tianshell_core::store::{HostStore, SettingsStore, SnippetStore};
use tianshell_core::types::{AppSettings, AuthType, HostRecord, Snippet};
use tianshell_core::vault::{load_or_create_master_key, open, seal};

#[test]
fn test_vault_key_and_crypto() {
    let dir = tempdir().unwrap();
    let key_file = dir.path().join("vault.key");

    let key1 = load_or_create_master_key(&key_file).unwrap();
    assert_eq!(key1.len(), 32);

    let key2 = load_or_create_master_key(&key_file).unwrap();
    assert_eq!(key1, key2);

    let secret = "super-secret-password-123!";
    let sealed = seal(&key1, secret).unwrap();
    assert!(sealed.starts_with("v1:"));

    let opened = open(&key1, &sealed).unwrap();
    assert_eq!(opened, secret);

    let tampered = format!("{}tampered", &sealed[..sealed.len() - 4]);
    assert!(open(&key1, &tampered).is_err());
}

#[test]
fn test_host_store_crud_and_cycle() {
    let dir = tempdir().unwrap();
    let key_file = dir.path().join("vault.key");
    let hosts_file = dir.path().join("hosts.json");

    let key = load_or_create_master_key(&key_file).unwrap();
    let mut store = HostStore::load(&hosts_file, key).unwrap();

    let h1 = store
        .save(HostRecord {
            id: String::new(),
            alias: "bastion".to_string(),
            host: "1.1.1.1".to_string(),
            port: 22,
            username: "root".to_string(),
            auth_type: AuthType::Password,
            secret: Some(store.seal_secret("pw1").unwrap()),
            passphrase: None,
            group: None,
            jump_host_id: None,
            tags: vec!["jump".to_string()],
            note: None,
            known_fingerprint: None,
            created_at: String::new(),
            updated_at: String::new(),
        })
        .unwrap();
    assert!(!h1.id.is_empty());

    let h2 = store
        .save(HostRecord {
            id: String::new(),
            alias: "inner-db".to_string(),
            host: "10.0.0.2".to_string(),
            port: 22,
            username: "pg".to_string(),
            auth_type: AuthType::Password,
            secret: Some(store.seal_secret("pw2").unwrap()),
            passphrase: None,
            group: None,
            jump_host_id: Some(h1.id.clone()),
            tags: vec![],
            note: None,
            known_fingerprint: None,
            created_at: String::new(),
            updated_at: String::new(),
        })
        .unwrap();

    assert_eq!(store.list().len(), 2);

    // 尝试将 h1 的 jump_host_id 指向 h2，制造环路，应报错
    let mut cyclic_h1 = h1.clone();
    cyclic_h1.jump_host_id = Some(h2.id.clone());
    assert!(store.save(cyclic_h1).is_err());

    // 删除测试
    assert!(store.delete(&h2.id).unwrap());
    assert_eq!(store.list().len(), 1);
}

#[test]
fn test_snippet_and_settings_store() {
    let dir = tempdir().unwrap();
    let s_file = dir.path().join("snippets.json");
    let cfg_file = dir.path().join("settings.json");

    let mut snippets = SnippetStore::load(&s_file).unwrap();
    let s = snippets
        .save(Snippet {
            id: String::new(),
            name: "Check disk".to_string(),
            category: "disk".to_string(),
            command: "df -h".to_string(),
            created_at: String::new(),
        })
        .unwrap();
    assert_eq!(snippets.list().len(), 1);
    assert_eq!(snippets.list()[0].id, s.id);

    let mut settings = SettingsStore::load(&cfg_file).unwrap();
    assert_eq!(settings.get().theme, "dark");
    settings
        .update(AppSettings {
            monitor_interval_ms: 1000,
            theme: "light".to_string(),
            font_size: 16,
        })
        .unwrap();
    assert_eq!(settings.get().theme, "light");
}
