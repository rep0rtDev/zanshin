//! Fine-grained admin permissions: `resource:action` grants carried by console users and admin
//! API keys. Legacy `Role`s map onto permission sets so existing keys keep working unchanged.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::apikey::Role;

/// Everything the admin API / console can touch. The first path segment under `/admin/v1` maps
/// onto one of these (see [`resource_for_path`]).
pub const RESOURCES: &[&str] = &[
    "flags",
    "audiences",
    "experiments",
    "metrics",
    "live_events",
    "messages",
    "templates",
    "integrations",
    "webhooks",
    "journeys",
    "identities",
    "taxonomy",
    "analytics",
    "data",
    "audit",
    "api_keys",
    "users",
    "settings",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Action {
    Read,
    Write,
    Delete,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Read => "read",
            Action::Write => "write",
            Action::Delete => "delete",
        }
    }
    pub fn for_method(method: &str) -> Action {
        match method {
            "GET" | "HEAD" | "OPTIONS" => Action::Read,
            "DELETE" => Action::Delete,
            _ => Action::Write,
        }
    }
}

/// `{"flags": ["read", "write"], "audiences": ["read"]}`; the special key `"*"` grants the listed
/// actions on every resource. [`Permissions::all`] is `{"*": ["read","write","delete"]}`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Permissions(pub BTreeMap<String, BTreeSet<Action>>);

const ALL: [Action; 3] = [Action::Read, Action::Write, Action::Delete];

impl Permissions {
    pub fn all() -> Self {
        let mut m = BTreeMap::new();
        m.insert("*".to_string(), ALL.into_iter().collect());
        Self(m)
    }

    pub fn read_all() -> Self {
        let mut m = BTreeMap::new();
        m.insert("*".to_string(), [Action::Read].into_iter().collect());
        Self(m)
    }

    pub fn is_empty(&self) -> bool {
        self.0.values().all(|a| a.is_empty())
    }

    pub fn grant(mut self, resource: &str, actions: &[Action]) -> Self {
        self.0
            .entry(resource.to_string())
            .or_default()
            .extend(actions.iter().copied());
        self
    }

    pub fn allows(&self, resource: &str, action: Action) -> bool {
        // `delete` implies nothing extra; `write` holders may also read.
        let has = |set: &BTreeSet<Action>| {
            set.contains(&action)
                || (action == Action::Read
                    && (set.contains(&Action::Write) || set.contains(&Action::Delete)))
        };
        // A resource-specific entry overrides the `*` wildcard, so presets can narrow it.
        match self.0.get(resource) {
            Some(set) => has(set),
            None => self.0.get("*").is_some_and(has),
        }
    }

    pub fn allows_any(&self, resource: &str) -> bool {
        self.allows(resource, Action::Read)
    }

    /// The permission set a legacy role stands for.
    pub fn from_role(role: Role) -> Self {
        match role {
            Role::Admin => Self::all(),
            Role::Editor => {
                let mut p = Self::all();
                p.0.insert("api_keys".into(), [Action::Read].into_iter().collect());
                p.0.insert("users".into(), [Action::Read].into_iter().collect());
                p
            }
            Role::Viewer => Self::read_all(),
        }
    }

    /// The closest legacy role (what older clients and `read_only` flags see).
    pub fn legacy_role(&self) -> Role {
        if self.allows("api_keys", Action::Write) && self.allows("flags", Action::Write) {
            Role::Admin
        } else if RESOURCES.iter().any(|r| self.allows(r, Action::Write)) {
            Role::Editor
        } else {
            Role::Viewer
        }
    }

    pub fn can_write_anything(&self) -> bool {
        RESOURCES
            .iter()
            .any(|r| self.allows(r, Action::Write) || self.allows(r, Action::Delete))
    }

    /// Resource names that are actually in [`RESOURCES`] (plus `*`).
    pub fn validate(&self) -> Result<(), String> {
        for k in self.0.keys() {
            if k != "*" && !RESOURCES.contains(&k.as_str()) {
                return Err(format!("unknown resource {k:?}"));
            }
        }
        Ok(())
    }
}

/// Named permission presets shown in the console.
pub struct Archetype {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

pub const ARCHETYPES: &[Archetype] = &[
    Archetype { name: "admin", title: "Admin", description: "Everything, including users and API keys." },
    Archetype { name: "developer", title: "Developer", description: "Flags, taxonomy, webhooks, integrations, data pipelines and API keys; reads everything else." },
    Archetype { name: "game_designer", title: "Game Designer", description: "LiveOps content: flags, audiences, experiments, metrics, live events, messaging, journeys." },
    Archetype { name: "debugger", title: "Debugger", description: "Reads everything; edits identities (QA time travel, properties) and the taxonomy debugger." },
    Archetype { name: "support", title: "Support", description: "Player lookup, inbox and direct messages; reads LiveOps configuration." },
    Archetype { name: "analyst", title: "Analyst", description: "Analytics, dashboards and exports; reads LiveOps configuration." },
    Archetype { name: "viewer", title: "Viewer", description: "Read-only everywhere." },
];

pub fn archetype(name: &str) -> Option<Permissions> {
    let w = &[Action::Read, Action::Write];
    let wd = &[Action::Read, Action::Write, Action::Delete];
    Some(match name {
        "admin" => Permissions::all(),
        "viewer" => Permissions::read_all(),
        "developer" => Permissions::read_all()
            .grant("flags", wd)
            .grant("taxonomy", wd)
            .grant("webhooks", wd)
            .grant("integrations", wd)
            .grant("data", wd)
            .grant("api_keys", wd)
            .grant("settings", w),
        "game_designer" => Permissions::read_all()
            .grant("flags", wd)
            .grant("audiences", wd)
            .grant("experiments", wd)
            .grant("metrics", wd)
            .grant("live_events", wd)
            .grant("messages", wd)
            .grant("templates", wd)
            .grant("journeys", wd),
        "debugger" => Permissions::read_all()
            .grant("identities", wd)
            .grant("taxonomy", w),
        "support" => Permissions::read_all()
            .grant("identities", w)
            .grant("messages", w),
        "analyst" => Permissions::read_all()
            .grant("analytics", wd)
            .grant("data", w),
        _ => return None,
    })
}

/// Map an admin API path (with or without the `/admin/v1` prefix) to its resource.
/// `None` = no resource check beyond "is an admin principal" (`/me`).
pub fn resource_for_path(path: &str) -> Option<&'static str> {
    let rest = path
        .strip_prefix("/admin/v1")
        .unwrap_or(path)
        .trim_start_matches('/');
    let first = rest.split('/').next().unwrap_or("");
    Some(match first {
        "me" => return None,
        "flags" => "flags",
        "audiences" => "audiences",
        "experiments" => "experiments",
        "metrics" => "metrics",
        "live-events" => "live_events",
        "messages" | "push" => "messages",
        "templates" => "templates",
        "integrations" => "integrations",
        "webhooks" => "webhooks",
        "journeys" => "journeys",
        "identities" => "identities",
        "taxonomy" | "labels" | "schemas" => "taxonomy",
        "analytics" | "dashboards" | "stats" => "analytics",
        "data" | "export-sinks" | "config" => "data",
        "audit" => "audit",
        "api-keys" => "api_keys",
        "users" => "users",
        "system" | "permissions" => "settings",
        _ => "settings",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_and_archetypes() {
        let admin = Permissions::from_role(Role::Admin);
        assert!(admin.allows("api_keys", Action::Delete));
        let editor = Permissions::from_role(Role::Editor);
        assert!(editor.allows("flags", Action::Write));
        assert!(!editor.allows("api_keys", Action::Write));
        assert_eq!(editor.legacy_role(), Role::Editor);
        let viewer = Permissions::from_role(Role::Viewer);
        assert!(viewer.allows("flags", Action::Read));
        assert!(!viewer.allows("flags", Action::Write));
        assert_eq!(viewer.legacy_role(), Role::Viewer);
        let gd = archetype("game_designer").unwrap();
        assert!(gd.allows("flags", Action::Delete));
        assert!(!gd.allows("api_keys", Action::Write));
        assert!(gd.allows("api_keys", Action::Read));
        assert_eq!(
            resource_for_path("/admin/v1/live-events/x/join"),
            Some("live_events")
        );
        assert_eq!(resource_for_path("/api-keys"), Some("api_keys"));
        assert_eq!(resource_for_path("/admin/v1/me"), None);
        let json = serde_json::to_string(&gd).unwrap();
        let back: Permissions = serde_json::from_str(&json).unwrap();
        assert_eq!(back, gd);
    }
}
