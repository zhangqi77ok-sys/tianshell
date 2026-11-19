//! Which open tab a coding tool should use.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct TerminalView {
    #[serde(rename = "sessionId")]
    pub session_id: String,
    pub kind: String,
    pub alias: String,
    pub username: String,
    pub host: String,
    pub port: u16,
    #[serde(rename = "hostId", skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(rename = "lastExit", skip_serializing_if = "Option::is_none")]
    pub last_exit: Option<i32>,
    pub busy: bool,
}

#[derive(Debug)]
pub enum TerminalPick {
    One(TerminalView),
    None,
    Many(Vec<TerminalView>),
    Missing,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunRoute {
    Attach(String),
    Dial,
    Ambiguous(Vec<String>),
    NoSuchSession,
}

pub fn is_local_host(host: Option<&str>) -> bool {
    matches!(
        host.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_ascii_lowercase()),
        Some(ref value) if value == "local" || value == "localhost"
    )
}

pub fn pick_terminal(tabs: &[TerminalView], session_id: Option<&str>, host: Option<&str>) -> TerminalPick {
    if let Some(id) = session_id.map(str::trim).filter(|s| !s.is_empty()) {
        return match tabs.iter().find(|tab| tab.session_id == id) {
            Some(tab) => TerminalPick::One(tab.clone()),
            None => TerminalPick::Missing,
        };
    }
    let Some(host) = host.map(str::trim).filter(|s| !s.is_empty()) else {
        return TerminalPick::None;
    };
    let matched: Vec<TerminalView> = tabs.iter().filter(|tab| tab_matches(tab, host)).cloned().collect();
    match matched.len() {
        0 => TerminalPick::None,
        1 => TerminalPick::One(matched.into_iter().next().unwrap()),
        _ => TerminalPick::Many(matched),
    }
}

pub fn route_run(tabs: &[TerminalView], session_id: Option<&str>, host: Option<&str>, new_session: bool) -> RunRoute {
    if new_session {
        return RunRoute::Dial;
    }
    match pick_terminal(tabs, session_id, host) {
        TerminalPick::One(tab) => RunRoute::Attach(tab.session_id),
        TerminalPick::None => RunRoute::Dial,
        TerminalPick::Many(tabs) => RunRoute::Ambiguous(tabs.into_iter().map(|tab| tab.session_id).collect()),
        TerminalPick::Missing => RunRoute::NoSuchSession,
    }
}

fn tab_matches(tab: &TerminalView, host: &str) -> bool {
    if is_local_host(Some(host)) {
        return tab.kind == "local";
    }
    if tab.kind != "ssh" {
        return false;
    }
    tab.alias.eq_ignore_ascii_case(host)
        || tab.host.eq_ignore_ascii_case(host)
        || tab.host_id.as_ref().map(|id| id.eq_ignore_ascii_case(host)).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tab(id: &str, kind: &str, alias: &str) -> TerminalView {
        TerminalView {
            session_id: id.into(),
            kind: kind.into(),
            alias: alias.into(),
            username: "ops".into(),
            host: if kind == "local" { "localhost".into() } else { "10.0.0.8".into() },
            port: if kind == "local" { 0 } else { 22 },
            host_id: if kind == "ssh" { Some(format!("id-{alias}")) } else { None },
            cwd: Some("/srv".into()),
            last_exit: Some(0),
            busy: false,
        }
    }

    #[test]
    fn attach_is_preferred_until_the_caller_asks_for_a_new_session() {
        let tabs = vec![tab("s1", "ssh", "lab"), tab("s2", "local", "本机终端")];
        assert_eq!(route_run(&tabs, None, Some("lab"), false), RunRoute::Attach("s1".into()));
        assert_eq!(route_run(&tabs, None, Some("LAB"), false), RunRoute::Attach("s1".into()));
        assert_eq!(route_run(&tabs, None, Some("id-lab"), false), RunRoute::Attach("s1".into()));
        assert_eq!(route_run(&tabs, None, Some("local"), false), RunRoute::Attach("s2".into()));
        assert_eq!(route_run(&tabs, Some("s1"), Some("other"), false), RunRoute::Attach("s1".into()));
        assert_eq!(route_run(&tabs, None, Some("lab"), true), RunRoute::Dial);
        assert_eq!(route_run(&tabs, None, Some("missing"), false), RunRoute::Dial);
        assert!(matches!(route_run(&tabs, Some("nope"), None, false), RunRoute::NoSuchSession));
        let two = vec![tab("a", "ssh", "lab"), tab("b", "ssh", "lab")];
        match route_run(&two, None, Some("lab"), false) {
            RunRoute::Ambiguous(ids) => assert_eq!(ids.len(), 2),
            other => panic!("expected ambiguous, got {other:?}"),
        }
    }
}
