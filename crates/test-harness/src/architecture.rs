//! Workspace dependency-boundary checker.
//!
//! The allowlist below is the executable form of the package table in the CLI
//! foundation system specification. Changing it is an architectural decision: edit it
//! together with that table.
//!
//! The functions are pure over [`cargo_metadata::Metadata`] (as produced by
//! `cargo metadata --no-deps`) and perform no I/O. Edges are identified by the
//! dependency `path`, matched lexically to a member's manifest directory. Every declared
//! edge counts, whatever its kind, `optional`, `rename`, or `target`.

use cargo_metadata::{DependencyKind, Metadata};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

/// Each workspace member and the only workspace members it may depend on.
pub const ALLOWED: &[(&str, &[&str])] = &[
    ("winter-platform", &[]),
    ("winter-cli", &["winter-platform"]),
    ("winter-test-harness", &["winter-platform"]),
    ("winter-dev-sample", &[]),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Normal,
    Dev,
    Build,
    Unknown,
}
impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Normal => "normal",
            Kind::Dev => "dev",
            Kind::Build => "build",
            Kind::Unknown => "unknown kind",
        })
    }
}

/// One forbidden workspace dependency edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyViolation {
    pub from_package: String,
    /// The target member, or the unresolved path when it is no member.
    pub to_package: String,
    pub dependency_kind: Kind,
    pub target_condition: Option<String>,
    pub dependency_alias: Option<String>,
    pub optional: bool,
}
impl fmt::Display for DependencyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "forbidden workspace dependency: {} -> {} ({}",
            self.from_package, self.to_package, self.dependency_kind
        )?;
        if self.optional {
            f.write_str(", optional")?;
        }
        if let Some(t) = &self.target_condition {
            write!(f, ", target {t}")?;
        }
        if let Some(a) = &self.dependency_alias {
            write!(f, ", alias {a}")?;
        }
        f.write_str(")")
    }
}

/// The workspace does not contain exactly the allowlisted members.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MembershipError {
    pub missing: Vec<String>,
    pub unexpected: Vec<String>,
}
impl fmt::Display for MembershipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "workspace members differ from the allowlist: missing [{}], unexpected [{}]",
            self.missing.join(", "),
            self.unexpected.join(", ")
        )
    }
}

fn normalize(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." if parts.last().is_some_and(|p| *p != "..") => {
                parts.pop();
            }
            p => parts.push(p),
        }
    }
    parts.join("/")
}

fn members(meta: &Metadata) -> impl Iterator<Item = &cargo_metadata::Package> {
    meta.packages
        .iter()
        .filter(|p| meta.workspace_members.contains(&p.id))
}

/// Requires the workspace to contain exactly the allowlisted members.
pub fn check_workspace_members(meta: &Metadata) -> Result<(), MembershipError> {
    let actual: BTreeSet<&str> = members(meta).map(|p| p.name.as_str()).collect();
    let expected: BTreeSet<&str> = ALLOWED.iter().map(|(n, _)| *n).collect();
    let missing: Vec<String> = expected
        .difference(&actual)
        .map(|s| s.to_string())
        .collect();
    let unexpected: Vec<String> = actual
        .difference(&expected)
        .map(|s| s.to_string())
        .collect();
    // A name listed twice in the metadata is also drift.
    let duplicated = members(meta).count() != actual.len();
    if missing.is_empty() && unexpected.is_empty() && !duplicated {
        Ok(())
    } else {
        Err(MembershipError {
            missing,
            unexpected,
        })
    }
}

/// Returns every declared workspace edge outside the allowlist.
pub fn check_workspace_boundaries(meta: &Metadata) -> Result<(), Vec<DependencyViolation>> {
    let dirs: BTreeMap<String, &str> = members(meta)
        .filter_map(|p| {
            let dir = p.manifest_path.as_str().rsplit_once('/')?.0;
            Some((normalize(dir), p.name.as_str()))
        })
        .collect();
    let mut violations = Vec::new();
    for pkg in members(meta) {
        let allowed: &[&str] = ALLOWED
            .iter()
            .find(|(n, _)| *n == pkg.name.as_str())
            .map_or(&[], |(_, a)| a);
        for dep in &pkg.dependencies {
            // Registry and git dependencies are not workspace edges.
            let Some(path) = &dep.path else { continue };
            let target = dirs.get(&normalize(path.as_str())).copied();
            let kind = match dep.kind {
                DependencyKind::Normal => Kind::Normal,
                DependencyKind::Development => Kind::Dev,
                DependencyKind::Build => Kind::Build,
                _ => Kind::Unknown,
            };
            let permitted = kind != Kind::Unknown && target.is_some_and(|t| allowed.contains(&t));
            if !permitted {
                violations.push(DependencyViolation {
                    from_package: pkg.name.to_string(),
                    to_package: target.map_or_else(|| path.to_string(), str::to_owned),
                    dependency_kind: kind,
                    target_condition: dep.target.as_ref().map(|t| t.to_string()),
                    dependency_alias: dep.rename.clone(),
                    optional: dep.optional,
                });
            }
        }
    }
    if violations.is_empty() {
        Ok(())
    } else {
        Err(violations)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const ROOT: &str = "/ws";
    fn dir(name: &str) -> String {
        format!(
            "{ROOT}/crates/{}",
            name.strip_prefix("winter-").unwrap_or(name)
        )
    }
    fn id(name: &str) -> String {
        format!("path+file://{}#{name}@0.1.0", dir(name))
    }
    fn dep(to: &str) -> Value {
        json!({
            "name": to, "source": null, "req": "*", "kind": null, "rename": null,
            "optional": false, "uses_default_features": false, "features": [],
            "target": null, "registry": null, "path": dir(to)
        })
    }
    fn registry(name: &str) -> Value {
        json!({
            "name": name, "source": "registry+https://github.com/rust-lang/crates.io-index",
            "req": "^1", "kind": null, "rename": null, "optional": false,
            "uses_default_features": true, "features": [], "target": null, "registry": null
        })
    }
    fn package(name: &str, deps: Vec<Value>) -> Value {
        json!({
            "name": name, "version": "0.1.0", "id": id(name), "license": null,
            "license_file": null, "description": null, "source": null,
            "dependencies": deps, "targets": [], "features": {},
            "manifest_path": format!("{}/Cargo.toml", dir(name)), "metadata": null,
            "publish": null, "authors": [], "categories": [], "keywords": [],
            "readme": null, "repository": null, "homepage": null,
            "documentation": null, "edition": "2024", "links": null,
            "default_run": null, "rust_version": "1.99"
        })
    }
    fn baseline() -> Vec<Value> {
        vec![
            package("winter-platform", vec![registry("clap")]),
            package("winter-cli", vec![dep("winter-platform")]),
            package(
                "winter-test-harness",
                vec![dep("winter-platform"), registry("serde_json")],
            ),
            package("winter-dev-sample", vec![registry("serde_json")]),
        ]
    }
    fn metadata(packages: Vec<Value>) -> Metadata {
        let ids: Vec<Value> = packages.iter().map(|p| p["id"].clone()).collect();
        serde_json::from_value(json!({
            "packages": packages, "workspace_members": ids,
            "workspace_default_members": ids, "resolve": null,
            "target_directory": "/ws/target", "build_directory": "/ws/target",
            "version": 1, "workspace_root": ROOT, "metadata": null
        }))
        .expect("fixture is valid cargo metadata")
    }
    // Replaces one member's dependency list.
    fn with(member: &str, deps: Vec<Value>) -> Metadata {
        let mut pkgs = baseline();
        for p in &mut pkgs {
            if p["name"] == member {
                p["dependencies"] = Value::Array(deps.clone());
            }
        }
        metadata(pkgs)
    }
    fn edit(mut d: Value, f: impl FnOnce(&mut Value)) -> Value {
        f(&mut d);
        d
    }
    fn violations(meta: &Metadata) -> Vec<DependencyViolation> {
        check_workspace_boundaries(meta).expect_err("expected violations")
    }
    fn one(meta: &Metadata) -> DependencyViolation {
        let mut v = violations(meta);
        assert_eq!(v.len(), 1, "{v:?}");
        v.remove(0)
    }
    fn violation(
        from: &str,
        to: &str,
        kind: Kind,
        target: Option<&str>,
        alias: Option<&str>,
        optional: bool,
    ) -> DependencyViolation {
        DependencyViolation {
            from_package: from.into(),
            to_package: to.into(),
            dependency_kind: kind,
            target_condition: target.map(Into::into),
            dependency_alias: alias.map(Into::into),
            optional,
        }
    }

    #[test]
    fn real_allowlist_edges_pass() {
        assert_eq!(check_workspace_boundaries(&metadata(baseline())), Ok(()));
        assert_eq!(check_workspace_members(&metadata(baseline())), Ok(()));
    }

    #[test]
    fn registry_and_git_dependencies_are_not_workspace_edges() {
        // Even a registry or git dependency sharing a member's name is ignored.
        let mut git = registry("winter-test-harness");
        git["source"] = json!("git+https://example.invalid/x");
        let meta = with(
            "winter-cli",
            vec![dep("winter-platform"), registry("winter-test-harness"), git],
        );
        assert_eq!(check_workspace_boundaries(&meta), Ok(()));
    }

    #[test]
    fn allowlist_is_acyclic() {
        // Every cycle needs an edge outside an acyclic allowlist, so one is enough to reject cycles.
        let mut done: BTreeSet<&str> = BTreeSet::new();
        while done.len() < ALLOWED.len() {
            let before = done.len();
            for (name, deps) in ALLOWED {
                if deps.iter().all(|d| done.contains(d)) {
                    done.insert(name);
                }
            }
            assert!(done.len() > before, "allowlist contains a cycle");
        }
        for (_, deps) in ALLOWED {
            assert!(deps.iter().all(|d| ALLOWED.iter().any(|(n, _)| n == d)));
        }
    }

    #[test]
    fn forbidden_normal_edge() {
        let meta = with(
            "winter-cli",
            vec![dep("winter-platform"), dep("winter-test-harness")],
        );
        let v = one(&meta);
        assert_eq!(
            v,
            violation(
                "winter-cli",
                "winter-test-harness",
                Kind::Normal,
                None,
                None,
                false
            )
        );
        assert_eq!(
            v.to_string(),
            "forbidden workspace dependency: winter-cli -> winter-test-harness (normal)"
        );
    }

    #[test]
    fn forbidden_optional_edge() {
        let meta = with(
            "winter-platform",
            vec![edit(dep("winter-dev-sample"), |d| {
                d["optional"] = json!(true)
            })],
        );
        let v = one(&meta);
        assert_eq!(
            v,
            violation(
                "winter-platform",
                "winter-dev-sample",
                Kind::Normal,
                None,
                None,
                true
            )
        );
        assert_eq!(
            v.to_string(),
            "forbidden workspace dependency: winter-platform -> winter-dev-sample (normal, optional)"
        );
    }

    #[test]
    fn forbidden_renamed_edge() {
        let meta = with(
            "winter-cli",
            vec![
                dep("winter-platform"),
                edit(dep("winter-test-harness"), |d| {
                    d["rename"] = json!("harness")
                }),
            ],
        );
        let v = one(&meta);
        assert_eq!(
            v,
            violation(
                "winter-cli",
                "winter-test-harness",
                Kind::Normal,
                None,
                Some("harness"),
                false
            )
        );
        assert!(v.to_string().ends_with("(normal, alias harness)"));
    }

    #[test]
    fn forbidden_build_edge() {
        let meta = with(
            "winter-dev-sample",
            vec![edit(dep("winter-platform"), |d| d["kind"] = json!("build"))],
        );
        assert_eq!(
            one(&meta),
            violation(
                "winter-dev-sample",
                "winter-platform",
                Kind::Build,
                None,
                None,
                false
            )
        );
    }

    #[test]
    fn forbidden_dev_edge_that_cargo_allows_as_a_cycle() {
        let meta = with(
            "winter-platform",
            vec![edit(dep("winter-test-harness"), |d| {
                d["kind"] = json!("dev")
            })],
        );
        let v = one(&meta);
        assert_eq!(
            v,
            violation(
                "winter-platform",
                "winter-test-harness",
                Kind::Dev,
                None,
                None,
                false
            )
        );
        assert_eq!(
            v.to_string(),
            "forbidden workspace dependency: winter-platform -> winter-test-harness (dev)"
        );
    }

    #[test]
    fn forbidden_target_specific_edge() {
        let meta = with(
            "winter-cli",
            vec![
                dep("winter-platform"),
                edit(dep("winter-dev-sample"), |d| {
                    d["target"] = json!("cfg(windows)")
                }),
            ],
        );
        let v = one(&meta);
        assert_eq!(
            v,
            violation(
                "winter-cli",
                "winter-dev-sample",
                Kind::Normal,
                Some("cfg(windows)"),
                None,
                false
            )
        );
        assert!(v.to_string().ends_with("(normal, target cfg(windows))"));
    }

    #[test]
    fn combined_optional_renamed_target_edge_names_every_condition() {
        let meta = with(
            "winter-cli",
            vec![
                dep("winter-platform"),
                edit(dep("winter-test-harness"), |d| {
                    d["optional"] = json!(true);
                    d["rename"] = json!("harness");
                    d["target"] = json!("cfg(windows)");
                }),
            ],
        );
        assert_eq!(
            one(&meta).to_string(),
            "forbidden workspace dependency: winter-cli -> winter-test-harness (normal, optional, target cfg(windows), alias harness)"
        );
    }

    #[test]
    fn path_dependency_outside_the_members_is_forbidden() {
        let meta = with(
            "winter-cli",
            vec![
                dep("winter-platform"),
                edit(dep("winter-platform"), |d| {
                    d["name"] = json!("stranger");
                    d["path"] = json!("/ws/crates/stranger");
                }),
            ],
        );
        let v = one(&meta);
        assert_eq!(v.from_package, "winter-cli");
        assert_eq!(v.to_package, "/ws/crates/stranger");
    }

    #[test]
    fn non_normalized_paths_still_resolve_to_members() {
        let meta = with(
            "winter-cli",
            vec![edit(dep("winter-platform"), |d| {
                d["path"] = json!("/ws/crates/cli/../platform/./")
            })],
        );
        assert_eq!(check_workspace_boundaries(&meta), Ok(()));
        let meta = with(
            "winter-cli",
            vec![edit(dep("winter-test-harness"), |d| {
                d["path"] = json!("/ws/crates/cli/../test-harness")
            })],
        );
        assert_eq!(one(&meta).to_package, "winter-test-harness");
    }

    #[test]
    fn unknown_dependency_kind_is_a_violation() {
        let meta = with(
            "winter-cli",
            vec![edit(dep("winter-platform"), |d| {
                d["kind"] = json!("mystery")
            })],
        );
        let v = one(&meta);
        assert_eq!(v.dependency_kind, Kind::Unknown);
        assert!(v.to_string().contains("unknown kind"));
    }

    #[test]
    fn every_violation_is_reported() {
        let meta = with(
            "winter-cli",
            vec![
                dep("winter-platform"),
                dep("winter-test-harness"),
                dep("winter-dev-sample"),
            ],
        );
        assert_eq!(violations(&meta).len(), 2);
    }

    #[test]
    fn extra_member_is_drift() {
        let mut pkgs = baseline();
        pkgs.push(package("winter-extra", vec![]));
        let err = check_workspace_members(&metadata(pkgs)).unwrap_err();
        assert_eq!(err.unexpected, ["winter-extra"]);
        assert!(err.missing.is_empty());
        assert_eq!(
            err.to_string(),
            "workspace members differ from the allowlist: missing [], unexpected [winter-extra]"
        );
    }

    #[test]
    fn missing_member_is_drift() {
        let mut pkgs = baseline();
        pkgs.retain(|p| p["name"] != "winter-dev-sample");
        let err = check_workspace_members(&metadata(pkgs)).unwrap_err();
        assert_eq!(err.missing, ["winter-dev-sample"]);
        assert!(err.unexpected.is_empty());
    }
}
