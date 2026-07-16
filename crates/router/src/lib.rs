//! `rayo-router` — radix-tree routing with typed path parameters, built at
//! startup from the routes the Python surface registers (never mutated while
//! serving, so it is shared across worker threads without locks).
//!
//! The radix tree itself is [`matchit`] (the router behind axum, MIT) per
//! ADR-0009: routing trees are solved infrastructure; Rayo's value is what
//! happens around them. Path parameters use `{name}` syntax end to end.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::fmt;

/// A successful lookup: which registered handler matched, and the raw path
/// parameter values captured from the URL (typed conversion happens at the
/// dispatch boundary, where a failure can become a 422 response).
pub struct RouteMatch {
    pub handler_id: usize,
    pub path_params: Vec<(String, String)>,
}

/// A route failed to register at startup — reported loudly with the route
/// that caused it (invariant 5: nothing defers config errors to request time).
#[derive(Debug)]
pub struct RouteRegistrationError {
    pub method: String,
    pub path: String,
    pub reason: String,
}

impl fmt::Display for RouteRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "cannot register route {} {}: {}",
            self.method, self.path, self.reason
        )
    }
}

impl std::error::Error for RouteRegistrationError {}

/// Immutable-after-startup routing table: one radix tree per HTTP method.
#[derive(Default)]
pub struct Router {
    trees_by_method: HashMap<String, matchit::Router<usize>>,
}

impl Router {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_route(
        &mut self,
        method: &str,
        path: &str,
        handler_id: usize,
    ) -> Result<(), RouteRegistrationError> {
        self.trees_by_method
            .entry(method.to_owned())
            .or_default()
            .insert(path, handler_id)
            .map_err(|insert_error| RouteRegistrationError {
                method: method.to_owned(),
                path: path.to_owned(),
                reason: insert_error.to_string(),
            })
    }

    pub fn lookup(&self, method: &str, path: &str) -> Option<RouteMatch> {
        let tree = self.trees_by_method.get(method)?;
        let matched = tree.at(path).ok()?;
        Some(RouteMatch {
            handler_id: *matched.value,
            path_params: matched
                .params
                .iter()
                .map(|(name, value)| (name.to_owned(), value.to_owned()))
                .collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn router_with(routes: &[(&str, &str)]) -> Router {
        let mut router = Router::new();
        for (index, (method, path)) in routes.iter().enumerate() {
            router
                .add_route(method, path, index)
                .unwrap_or_else(|registration_error| panic!("{registration_error}"));
        }
        router
    }

    #[test]
    fn matches_static_and_parameterized_paths() {
        let router = router_with(&[("GET", "/health"), ("GET", "/users/{user_id}")]);

        let static_match = router.lookup("GET", "/health");
        assert_eq!(static_match.map(|matched| matched.handler_id), Some(0));

        let Some(matched) = router.lookup("GET", "/users/42") else {
            panic!("parameterized route should match");
        };
        assert_eq!(matched.handler_id, 1);
        assert_eq!(
            matched.path_params,
            vec![("user_id".to_owned(), "42".to_owned())]
        );
    }

    #[test]
    fn misses_unknown_paths_and_methods() {
        let router = router_with(&[("GET", "/users/{user_id}")]);
        assert!(router.lookup("GET", "/missing").is_none());
        assert!(router.lookup("POST", "/users/42").is_none());
    }

    #[test]
    fn rejects_conflicting_routes_at_startup() {
        let mut router = router_with(&[("GET", "/users/{user_id}")]);
        let conflict = router.add_route("GET", "/users/{other_name}", 9);
        assert!(conflict.is_err());
    }
}
