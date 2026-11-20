use serde_json::json;
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
    #[cfg(not(windows))]
    {
        let bytes = std::fs::read(&key_file).unwrap();
        assert!(bytes.starts_with(b"TS1R"), "POSIX vault.key should use the Node TS1R raw format");
    }

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
            secret: store.seal_secret("pw1").unwrap(),
            passphrase: None,
            group_id: None,
            jump_host_id: None,
            tags: vec!["jump".to_string()],
            notes: None,
            fingerprint: None,
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
            secret: store.seal_secret("pw2").unwrap(),
            passphrase: None,
            group_id: None,
            jump_host_id: Some(h1.id.clone()),
            tags: vec![],
            notes: None,
            fingerprint: None,
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
            description: None,
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
            show_hidden_files: false,
        })
        .unwrap();
    assert_eq!(settings.get().theme, "light");
}

#[test]
fn loads_node_hosts_json_without_leaking_secrets() {
    let dir = tempdir().unwrap();
    let key = load_or_create_master_key(&dir.path().join("vault.key")).unwrap();
    let sealed = seal(&key, "s3cret").unwrap();
    let hosts_file = dir.path().join("hosts.json");
    std::fs::write(
        &hosts_file,
        format!(
            r#"{{
  "version": 1,
  "hosts": [{{
    "id": "h1",
    "alias": "box",
    "host": "10.0.0.8",
    "port": 22,
    "username": "root",
    "authType": "password",
    "secret": "{sealed}",
    "passphrase": null,
    "fingerprint": null,
    "groupId": null,
    "jumpHostId": null,
    "tags": ["lab"],
    "notes": "from node",
    "createdAt": "2020-01-01T00:00:00.000Z",
    "updatedAt": "2020-01-01T00:00:00.000Z"
  }}],
  "groups": [{{"id": "g1", "name": "研发", "parentId": null, "order": 1}}]
}}"#
        ),
    )
    .unwrap();
    let store = HostStore::load(&hosts_file, key).unwrap();
    assert_eq!(store.open_secret(&store.list()[0].secret).unwrap(), "s3cret");
    let public = serde_json::to_string(&store.list_public()).unwrap();
    assert!(public.contains("box"));
    assert!(!public.contains("s3cret"));
    assert!(!public.contains(&sealed));
    assert_eq!(store.list_groups_sorted()[0].name, "研发");
}

#[test]
fn test_group_rename_keeps_id_and_rejects_duplicates() {
    let dir = tempdir().unwrap();
    let key = load_or_create_master_key(&dir.path().join("vault.key")).unwrap();
    let mut store = HostStore::load(dir.path().join("hosts.json"), key).unwrap();
    let first = store.save_group("研发", None).unwrap();
    let err = store.save_group("研发", None).unwrap_err();
    assert!(err.to_string().contains("研发"));
    let renamed = store.save_group_draft(&json!({ "id": first.id, "name": "运维" })).unwrap();
    assert_eq!(renamed.id, first.id);
    assert_eq!(renamed.name, "运维");
    assert_eq!(store.list_groups().len(), 1);
}
