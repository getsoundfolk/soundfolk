// SPDX-License-Identifier: MPL-2.0
//! Sites the user adds to a track's "Find this track on" menu: a name and a search address with
//! `{query}`. Checked here before they are saved, whatever the frontend already checked.

use serde::{Deserialize, Serialize};

use crate::db::Database;
use crate::error::AppResult;

/// Most custom sites one can add: the menu must stay short.
pub const MAX_CUSTOM: usize = 10;
const MAX_NAME: usize = 40;
const PLACEHOLDER: &str = "{query}";

/// Services listed under "Find this track on", by id. Unset: the frontend's defaults.
const SETTING_TRACK_SERVICES: &str = "track_services";
/// Sites the user added to that menu (JSON list of `CustomService`).
const SETTING_CUSTOM_TRACK_SERVICES: &str = "custom_track_services";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomService {
    /// `custom:…`, stored among the enabled ids (a comma-separated list).
    pub id: String,
    pub name: String,
    /// Search address, `{query}` standing for the encoded track.
    pub template: String,
}

/// Checks a whole list and returns it with names trimmed, or the first problem found.
pub fn validate_all(services: Vec<CustomService>) -> Result<Vec<CustomService>, String> {
    if services.len() > MAX_CUSTOM {
        return Err(format!("At most {MAX_CUSTOM} sites can be added"));
    }
    let mut out: Vec<CustomService> = Vec::with_capacity(services.len());
    for s in services {
        let s = validate(s)?;
        if out.iter().any(|o| o.id == s.id) {
            return Err("Two sites have the same id".into());
        }
        out.push(s);
    }
    Ok(out)
}

pub fn validate(s: CustomService) -> Result<CustomService, String> {
    let id_ok = s.id.len() > "custom:".len()
        && s.id.len() <= 64
        && s.id.starts_with("custom:")
        && s.id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-');
    if !id_ok {
        return Err("Invalid site id".into());
    }
    let name = s.name.trim();
    if name.is_empty() || name.chars().count() > MAX_NAME {
        return Err(format!("The name must be 1 to {MAX_NAME} characters"));
    }
    let template = s.template.trim();
    if template.matches(PLACEHOLDER).count() != 1 {
        return Err("The address must contain {query} once".into());
    }
    // Checked on the text too: the parser reads `https:///search` as the host `search`.
    let Some(rest) = template.strip_prefix("https://") else {
        return Err("The address must start with https://".into());
    };
    if rest.starts_with('/') {
        return Err("The address needs a site name, without login".into());
    }
    let url = url::Url::parse(&template.replace(PLACEHOLDER, "test"))
        .map_err(|_| "This address is not valid".to_string())?;
    if url.scheme() != "https" {
        return Err("The address must start with https://".into());
    }
    if url.host_str().is_none_or(str::is_empty)
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err("The address needs a site name, without login".into());
    }
    Ok(CustomService {
        id: s.id,
        name: name.to_owned(),
        template: template.to_owned(),
    })
}

/// Ids of the services shown in a track's menu, or `None` if the user never chose.
pub fn enabled_ids(db: &Database) -> AppResult<Option<Vec<String>>> {
    Ok(db.get_list(SETTING_TRACK_SERVICES)?)
}

/// Saves the ids shown in the menu; ids that cannot be stored in the list are dropped.
pub fn save_enabled_ids(db: &Database, ids: Vec<String>) -> AppResult<()> {
    let ids: Vec<String> = ids
        .into_iter()
        .filter(|id| !id.is_empty() && !id.contains(','))
        .collect();
    Ok(db.set_list(SETTING_TRACK_SERVICES, &ids)?)
}

/// Sites the user added. A stored list that no longer passes the checks is dropped, not trusted.
pub fn load_custom(db: &Database) -> AppResult<Vec<CustomService>> {
    let Some(json) = db.get_str(SETTING_CUSTOM_TRACK_SERVICES)? else {
        return Ok(Vec::new());
    };
    let stored: Vec<CustomService> = serde_json::from_str(&json).unwrap_or_default();
    Ok(stored
        .into_iter()
        .filter_map(|s| validate(s).ok())
        .collect())
}

/// Replaces the whole list, once checked (see [`validate_all`]).
pub fn save_custom(db: &Database, services: &[CustomService]) -> AppResult<()> {
    let json = serde_json::to_string(services).map_err(|e| e.to_string())?;
    Ok(db.set_str(SETTING_CUSTOM_TRACK_SERVICES, &json)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site(name: &str, template: &str) -> CustomService {
        CustomService {
            id: "custom:abc123".into(),
            name: name.into(),
            template: template.into(),
        }
    }

    #[test]
    fn accepts_https_search_addresses() {
        assert!(validate(site("Invidious", "https://yewtu.be/search?q={query}")).is_ok());
        assert!(validate(site("Funkwhale", "https://nas.local:8443/search?q={query}")).is_ok());
        assert!(validate(site("PeerTube", "https://tube.example.org/search/{query}")).is_ok());
    }

    #[test]
    fn trims_the_name_and_address() {
        let s = validate(site("  Invidious ", " https://yewtu.be/search?q={query} ")).unwrap();
        assert_eq!(s.name, "Invidious");
        assert_eq!(s.template, "https://yewtu.be/search?q={query}");
    }

    #[test]
    fn refuses_other_schemes() {
        for t in [
            "http://nas.local/search?q={query}",
            "file:///etc/{query}",
            "javascript:alert('{query}')",
            "steam://run/{query}",
        ] {
            assert!(validate(site("x", t)).is_err(), "{t}");
        }
    }

    #[test]
    fn refuses_a_missing_or_repeated_placeholder() {
        assert!(validate(site("x", "https://yewtu.be/search?q=")).is_err());
        assert!(validate(site("x", "https://yewtu.be/{query}?q={query}")).is_err());
    }

    #[test]
    fn refuses_logins_and_missing_hosts() {
        assert!(validate(site("x", "https://user:pass@yewtu.be/search?q={query}")).is_err());
        assert!(validate(site("x", "https:///search?q={query}")).is_err());
        assert!(validate(site("x", "not an address {query}")).is_err());
    }

    #[test]
    fn refuses_bad_names_and_ids() {
        assert!(validate(site("   ", "https://yewtu.be/search?q={query}")).is_err());
        assert!(validate(site(&"a".repeat(41), "https://yewtu.be/search?q={query}")).is_err());
        let mut s = site("x", "https://yewtu.be/search?q={query}");
        s.id = "youtube".into();
        assert!(validate(s.clone()).is_err());
        s.id = "custom:a,b".into();
        assert!(validate(s).is_err());
    }

    #[test]
    fn refuses_too_many_or_duplicate_sites() {
        let ok = |i: usize| CustomService {
            id: format!("custom:{i}"),
            name: format!("Site {i}"),
            template: "https://yewtu.be/search?q={query}".into(),
        };
        assert!(validate_all((0..MAX_CUSTOM).map(ok).collect()).is_ok());
        assert!(validate_all((0..=MAX_CUSTOM).map(ok).collect()).is_err());
        assert!(validate_all(vec![ok(1), ok(1)]).is_err());
    }
}
