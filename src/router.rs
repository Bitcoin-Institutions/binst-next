//! Hash routes. Every screen has a URL, so Back works and links can be shared.

use crate::model::{is_evm_address, is_inscription_id};

#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    /// "My institutions": the connected wallet's trees, or the featured one.
    Institutions,
    /// One institution's tree, optionally with a definition selected.
    Institution { id: String, definition: Option<String> },
    /// An institution's members and roles (Nostr).
    Members { id: String },
    /// One process instance on Citrea.
    Instance { address: String },
    /// The create-institution flow.
    New,
    /// Where chain data comes from, and the one-time reveal keys.
    Settings,
    /// The Lab index, or one frontier feature.
    Lab(Option<String>),
    NotFound(String),
}

const LAB_PAGES: [&str; 4] = ["nostr", "r7", "ark", "ctv"];

/// Parse `location.hash` (with or without the leading `#`).
pub fn parse(hash: &str) -> Route {
    let path = hash.trim_start_matches('#').trim_start_matches('/').trim_end_matches('/');
    let parts: Vec<&str> = path.split('/').collect();
    match parts.as_slice() {
        [""] => Route::Home,
        ["institutions"] => Route::Institutions,
        ["new"] => Route::New,
        ["settings"] => Route::Settings,
        ["lab"] => Route::Lab(None),
        ["lab", page] if LAB_PAGES.contains(page) => Route::Lab(Some((*page).to_string())),
        ["i", id] if is_inscription_id(id) => Route::Institution { id: (*id).to_string(), definition: None },
        ["i", id, "members"] if is_inscription_id(id) => Route::Members { id: (*id).to_string() },
        ["i", id, "p", def] if is_inscription_id(id) && is_inscription_id(def) => {
            Route::Institution { id: (*id).to_string(), definition: Some((*def).to_string()) }
        }
        ["x", address] if is_evm_address(address) => Route::Instance { address: address.to_lowercase() },
        _ => Route::NotFound(path.chars().take(80).collect()),
    }
}

/// The `#/...` link for a route.
pub fn href(route: &Route) -> String {
    match route {
        Route::Home => "#/".to_string(),
        Route::Institutions => "#/institutions".to_string(),
        Route::New => "#/new".to_string(),
        Route::Settings => "#/settings".to_string(),
        Route::Lab(None) => "#/lab".to_string(),
        Route::Lab(Some(page)) => format!("#/lab/{page}"),
        Route::Institution { id, definition: None } => format!("#/i/{id}"),
        Route::Institution { id, definition: Some(def) } => format!("#/i/{id}/p/{def}"),
        Route::Members { id } => format!("#/i/{id}/members"),
        Route::Instance { address } => format!("#/x/{address}"),
        Route::NotFound(_) => "#/".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "46f5557a10e7a75ee0b03abea6c0ecaf9f74ce7ec7d3353359744cc518ca5226i0";
    const DEF: &str = "505eb39ed86053d5568a23bd128cd153142a21de02326e15354cb70059901e40i0";

    #[test]
    fn empty_hash_is_home() {
        assert_eq!(parse(""), Route::Home);
        assert_eq!(parse("#"), Route::Home);
        assert_eq!(parse("#/"), Route::Home);
    }

    #[test]
    fn routes_round_trip() {
        let routes = [
            Route::Home,
            Route::Institutions,
            Route::New,
            Route::Lab(None),
            Route::Lab(Some("nostr".into())),
            Route::Institution { id: ID.into(), definition: None },
            Route::Institution { id: ID.into(), definition: Some(DEF.into()) },
            Route::Members { id: ID.into() },
            Route::Instance { address: "0x6a1d2adbac8682773ed6700d2118c709c8ce5000".into() },
        ];
        for route in routes {
            assert_eq!(parse(&href(&route)), route);
        }
    }

    #[test]
    fn bad_ids_are_not_found() {
        assert!(matches!(parse("#/i/notanid"), Route::NotFound(_)));
        assert!(matches!(parse("#/x/0x123"), Route::NotFound(_)));
        assert!(matches!(parse("#/lab/unknown"), Route::NotFound(_)));
        assert!(matches!(parse("#/i/<img src=x>"), Route::NotFound(_)));
    }

    #[test]
    fn instance_addresses_are_lowercased() {
        assert_eq!(
            parse("#/x/0x6A1D2adbac8682773ed6700d2118c709c8ce5000"),
            Route::Instance { address: "0x6a1d2adbac8682773ed6700d2118c709c8ce5000".into() }
        );
    }
}
