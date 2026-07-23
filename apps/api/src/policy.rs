use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::error::{ApiError, ApiResult};

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PolicyDocument {
    pub policy_ref: String,
    pub version: i32,
    pub default_effect: Effect,
    #[serde(default)]
    pub rules: Vec<PolicyRule>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Allow,
    Deny,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PolicyRule {
    pub effect: Effect,
    #[serde(default)]
    pub principals: Vec<String>,
    #[serde(default)]
    pub purposes: Vec<String>,
    #[serde(default)]
    pub actions: Vec<String>,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct WithheldPath {
    pub path: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct ProjectionReceipt {
    pub policy_ref: String,
    pub policy_version: i32,
    pub audience: String,
    pub purpose: String,
    pub included_paths: Vec<String>,
    pub withheld: Vec<WithheldPath>,
    pub transforms: Vec<String>,
    pub output_hash: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct AuthorizedProjection {
    pub data: Value,
    pub receipt: ProjectionReceipt,
}

#[derive(Clone, Debug)]
pub struct ScopedPolicy {
    pub path: String,
    pub reason: String,
    pub policy: PolicyDocument,
}

#[derive(Default)]
struct ProjectionTrace {
    policy_refs: BTreeSet<String>,
    policy_version: i32,
    included_paths: Vec<String>,
    withheld: Vec<WithheldPath>,
    transforms: Vec<String>,
}

impl ProjectionTrace {
    fn add_policy(&mut self, policy: &PolicyDocument) {
        self.policy_refs
            .insert(format!("{}@v{}", policy.policy_ref, policy.version));
        self.policy_version = self.policy_version.max(policy.version);
    }

    fn merge(&mut self, other: Self) {
        self.policy_refs.extend(other.policy_refs);
        self.policy_version = self.policy_version.max(other.policy_version);
        self.included_paths.extend(other.included_paths);
        self.withheld.extend(other.withheld);
        self.transforms.extend(other.transforms);
    }

    fn merge_at(&mut self, path: &str, reason: &str, mut other: Self) {
        for included in &mut other.included_paths {
            *included = join_pointer(path, included);
        }
        for withheld in &mut other.withheld {
            withheld.path = join_pointer(path, &withheld.path);
            if withheld.reason.is_empty() {
                withheld.reason = reason.to_owned();
            }
        }
        self.merge(other);
    }

    fn normalize(&mut self) {
        self.included_paths.sort();
        self.included_paths.dedup();
        self.withheld.sort_by(|left, right| {
            left.path
                .cmp(&right.path)
                .then_with(|| left.reason.cmp(&right.reason))
        });
        self.withheld
            .dedup_by(|left, right| left.path == right.path && left.reason == right.reason);
        self.transforms.sort();
        self.transforms.dedup();
    }
}

pub fn project(
    value: Value,
    policies: &[PolicyDocument],
    audience: &str,
    purpose: &str,
    action: &str,
) -> ApiResult<AuthorizedProjection> {
    project_scoped(value, policies, &[], audience, purpose, action)
}

pub fn project_scoped(
    mut value: Value,
    policies: &[PolicyDocument],
    scoped_policies: &[ScopedPolicy],
    audience: &str,
    purpose: &str,
    action: &str,
) -> ApiResult<AuthorizedProjection> {
    if policies.is_empty() {
        return Err(policy_denied("protected data has no effective policy"));
    }

    let mut trace = ProjectionTrace::default();
    let mut scoped = scoped_policies.to_vec();
    scoped.sort_by(|left, right| pointer_removal_order(&left.path, &right.path));
    for scoped_policy in scoped {
        let Some(input) = value.pointer(&scoped_policy.path).cloned() else {
            continue;
        };
        match apply_policies(
            input,
            std::slice::from_ref(&scoped_policy.policy),
            audience,
            purpose,
            action,
        ) {
            Ok((projected, scoped_trace)) => {
                replace_pointer(&mut value, &scoped_policy.path, projected);
                trace.merge_at(&scoped_policy.path, &scoped_policy.reason, scoped_trace);
            }
            Err(error) if is_policy_denial(&error) && !scoped_policy.path.is_empty() => {
                trace.add_policy(&scoped_policy.policy);
                if remove_pointer(&mut value, &scoped_policy.path) {
                    trace.withheld.push(WithheldPath {
                        path: scoped_policy.path,
                        reason: scoped_policy.reason,
                    });
                }
            }
            Err(error) => return Err(error),
        }
    }

    let (value, global_trace) = apply_policies(value, policies, audience, purpose, action)?;
    trace.merge(global_trace);
    trace.normalize();
    let output = serde_json::to_vec(&value)?;
    Ok(AuthorizedProjection {
        data: value,
        receipt: ProjectionReceipt {
            policy_ref: trace
                .policy_refs
                .into_iter()
                .collect::<Vec<_>>()
                .join(" + "),
            policy_version: trace.policy_version.max(1),
            audience: audience.to_owned(),
            purpose: purpose.to_owned(),
            included_paths: trace.included_paths,
            withheld: trace.withheld,
            transforms: trace.transforms,
            output_hash: format!("sha256:{}", hex::encode(Sha256::digest(output))),
        },
    })
}

fn apply_policies(
    mut value: Value,
    policies: &[PolicyDocument],
    audience: &str,
    purpose: &str,
    action: &str,
) -> ApiResult<(Value, ProjectionTrace)> {
    let mut trace = ProjectionTrace::default();
    for policy in policies {
        trace.add_policy(policy);
        let mut allowed = Vec::new();
        let mut denied = Vec::new();
        for rule in &policy.rules {
            if matches_rule(rule, audience, purpose, action) {
                match rule.effect {
                    Effect::Allow => allowed.extend(rule.paths.iter().cloned()),
                    Effect::Deny => {
                        if rule.paths.is_empty() {
                            denied.push(String::new());
                        } else {
                            denied.extend(rule.paths.iter().cloned());
                        }
                    }
                }
            }
        }

        for pointer in allowed.iter().chain(&denied) {
            validate_pointer(pointer)?;
        }

        if policy.default_effect == Effect::Deny {
            if allowed.is_empty() {
                return Err(policy_denied("the effective policy denies this projection"));
            }
            allowed.sort();
            allowed.dedup();
            trace.included_paths.extend(allowed.iter().cloned());
            if !allowed.iter().any(String::is_empty) {
                let (restricted, withheld) = restrict_to_paths(&value, &allowed)?;
                value = restricted;
                trace.withheld.extend(withheld);
            }
        }

        denied.sort_by(|left, right| pointer_removal_order(left, right));
        denied.dedup();
        if denied.iter().any(String::is_empty) {
            return Err(policy_denied("the effective policy denies this projection"));
        }
        for pointer in denied {
            if remove_pointer(&mut value, &pointer) {
                trace.withheld.push(WithheldPath {
                    path: pointer,
                    reason: "field_policy".to_owned(),
                });
            }
        }
    }
    Ok((value, trace))
}

fn matches_rule(rule: &PolicyRule, audience: &str, purpose: &str, action: &str) -> bool {
    matches_dimension(&rule.principals, audience)
        && matches_dimension(&rule.purposes, purpose)
        && matches_dimension(&rule.actions, action)
}

fn matches_dimension(values: &[String], requested: &str) -> bool {
    values.is_empty() || values.iter().any(|item| item == "*" || item == requested)
}

#[derive(Default)]
struct PointerTrie {
    terminal: bool,
    children: BTreeMap<String, PointerTrie>,
}

fn restrict_to_paths(value: &Value, allowed: &[String]) -> ApiResult<(Value, Vec<WithheldPath>)> {
    let mut trie = PointerTrie::default();
    for pointer in allowed {
        let mut node = &mut trie;
        for segment in pointer_segments(pointer)? {
            node = node.children.entry(segment).or_default();
        }
        node.terminal = true;
    }
    let mut withheld = Vec::new();
    let projected = restrict_value(value, &trie, "", &mut withheld)?
        .ok_or_else(|| policy_denied("the effective policy does not allow a usable projection"))?;
    Ok((projected, withheld))
}

fn restrict_value(
    value: &Value,
    trie: &PointerTrie,
    path: &str,
    withheld: &mut Vec<WithheldPath>,
) -> ApiResult<Option<Value>> {
    if trie.terminal {
        return Ok(Some(value.clone()));
    }
    match value {
        Value::Object(object) => {
            let mut projected = Map::new();
            for (key, child) in object {
                let child_path = join_pointer(path, &format!("/{}", escape_pointer_segment(key)));
                if let Some(child_trie) = trie.children.get(key) {
                    if let Some(child) = restrict_value(child, child_trie, &child_path, withheld)? {
                        projected.insert(key.clone(), child);
                    } else {
                        withheld.push(WithheldPath {
                            path: child_path,
                            reason: "default_deny".to_owned(),
                        });
                    }
                } else {
                    withheld.push(WithheldPath {
                        path: child_path,
                        reason: "default_deny".to_owned(),
                    });
                }
            }
            Ok(Some(Value::Object(projected)))
        }
        Value::Array(_) if !trie.children.is_empty() => Err(policy_denied(
            "array-element allow lists are not supported by the bounded policy projector",
        )),
        _ => Ok(None),
    }
}

fn validate_pointer(pointer: &str) -> ApiResult<()> {
    pointer_segments(pointer).map(|_| ())
}

fn pointer_segments(pointer: &str) -> ApiResult<Vec<String>> {
    if pointer.is_empty() {
        return Ok(Vec::new());
    }
    let Some(pointer) = pointer.strip_prefix('/') else {
        return Err(policy_denied(
            "the effective policy contains an invalid JSON pointer",
        ));
    };
    pointer
        .split('/')
        .map(decode_pointer_segment)
        .collect::<ApiResult<Vec<_>>>()
}

fn decode_pointer_segment(segment: &str) -> ApiResult<String> {
    let mut decoded = String::with_capacity(segment.len());
    let mut characters = segment.chars();
    while let Some(character) = characters.next() {
        if character != '~' {
            decoded.push(character);
            continue;
        }
        match characters.next() {
            Some('0') => decoded.push('~'),
            Some('1') => decoded.push('/'),
            _ => {
                return Err(policy_denied(
                    "the effective policy contains an invalid JSON pointer",
                ));
            }
        }
    }
    Ok(decoded)
}

fn escape_pointer_segment(segment: &str) -> String {
    segment.replace('~', "~0").replace('/', "~1")
}

fn join_pointer(parent: &str, child: &str) -> String {
    match (parent.is_empty(), child.is_empty()) {
        (true, _) => child.to_owned(),
        (_, true) => parent.to_owned(),
        _ => format!("{parent}{child}"),
    }
}

fn pointer_removal_order(left: &str, right: &str) -> Ordering {
    let left = pointer_segments(left).unwrap_or_default();
    let right = pointer_segments(right).unwrap_or_default();
    right.len().cmp(&left.len()).then_with(|| {
        for (left, right) in left.iter().zip(&right) {
            let order = match (left.parse::<usize>(), right.parse::<usize>()) {
                (Ok(left), Ok(right)) => right.cmp(&left),
                _ => right.cmp(left),
            };
            if order != Ordering::Equal {
                return order;
            }
        }
        Ordering::Equal
    })
}

fn is_policy_denial(error: &ApiError) -> bool {
    matches!(
        error,
        ApiError::Public {
            code: "rejected_by_policy",
            ..
        }
    )
}

fn policy_denied(message: impl Into<String>) -> ApiError {
    ApiError::public(http::StatusCode::FORBIDDEN, "rejected_by_policy", message)
}

fn replace_pointer(value: &mut Value, pointer: &str, replacement: Value) -> bool {
    if pointer.is_empty() {
        *value = replacement;
        return true;
    }
    if let Some(target) = value.pointer_mut(pointer) {
        *target = replacement;
        true
    } else {
        false
    }
}

fn remove_pointer(value: &mut Value, pointer: &str) -> bool {
    let Some((parent_pointer, key)) = pointer.rsplit_once('/') else {
        return false;
    };
    let Ok(key) = decode_pointer_segment(key) else {
        return false;
    };
    let parent = if parent_pointer.is_empty() {
        value
    } else if let Some(parent) = value.pointer_mut(parent_pointer) {
        parent
    } else {
        return false;
    };
    match parent {
        Value::Object(object) => object.remove(&key).is_some(),
        Value::Array(array) => key
            .parse::<usize>()
            .ok()
            .filter(|index| *index < array.len())
            .map(|index| {
                array.remove(index);
                true
            })
            .unwrap_or(false),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn stricter_contributing_policy_redacts_field_and_receipts_it() {
        let policies = vec![PolicyDocument {
            policy_ref: "policy:dependent".to_owned(),
            version: 3,
            default_effect: Effect::Allow,
            rules: vec![PolicyRule {
                effect: Effect::Deny,
                principals: vec!["audience:public".to_owned()],
                purposes: vec!["event_listing".to_owned()],
                actions: vec!["read".to_owned()],
                paths: vec!["/contact".to_owned(), "/precise_handoff".to_owned()],
            }],
        }];
        let projection = project(
            json!({"date": "2026-12-12", "contact": "hidden", "precise_handoff": "hidden"}),
            &policies,
            "audience:public",
            "event_listing",
            "read",
        )
        .unwrap();
        assert!(projection.data.get("contact").is_none());
        assert_eq!(projection.receipt.withheld.len(), 2);
        assert!(projection.receipt.output_hash.starts_with("sha256:"));
    }

    #[test]
    fn wildcard_allow_rule_enforces_the_seeded_private_default() {
        let policy = PolicyDocument {
            policy_ref: "policy:private-default".to_owned(),
            version: 1,
            default_effect: Effect::Deny,
            rules: vec![PolicyRule {
                effect: Effect::Allow,
                principals: vec!["principal:self".to_owned()],
                purposes: vec!["*".to_owned()],
                actions: vec!["read".to_owned()],
                paths: vec![String::new()],
            }],
        };
        let projection = project(
            json!({"private": "visible to self"}),
            &[policy],
            "principal:self",
            "task_reasoning",
            "read",
        )
        .unwrap();
        assert_eq!(projection.data, json!({"private": "visible to self"}));
        assert_eq!(projection.receipt.included_paths, vec![""]);
    }

    #[test]
    fn default_deny_allow_list_withholds_every_other_object_path() {
        let policy = PolicyDocument {
            policy_ref: "policy:limited".to_owned(),
            version: 2,
            default_effect: Effect::Deny,
            rules: vec![PolicyRule {
                effect: Effect::Allow,
                principals: vec!["principal:self".to_owned()],
                purposes: vec!["task_reasoning".to_owned()],
                actions: vec!["read".to_owned()],
                paths: vec!["/event/date".to_owned()],
            }],
        };
        let projection = project(
            json!({
                "event": {"date": "2026-12-12", "contact": "hidden"},
                "internal": "hidden"
            }),
            &[policy],
            "principal:self",
            "task_reasoning",
            "read",
        )
        .unwrap();
        assert_eq!(projection.data, json!({"event": {"date": "2026-12-12"}}));
        assert_eq!(
            projection
                .receipt
                .withheld
                .iter()
                .map(|item| item.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/event/contact", "/internal"]
        );
    }

    #[test]
    fn multiple_array_denials_do_not_shift_a_withheld_value_into_output() {
        let policy = PolicyDocument {
            policy_ref: "policy:array".to_owned(),
            version: 1,
            default_effect: Effect::Allow,
            rules: vec![PolicyRule {
                effect: Effect::Deny,
                principals: vec![],
                purposes: vec![],
                actions: vec![],
                paths: vec!["/values/0".to_owned(), "/values/1".to_owned()],
            }],
        };
        let projection = project(
            json!({"values": ["first", "second", "third"]}),
            &[policy],
            "principal:self",
            "task_reasoning",
            "read",
        )
        .unwrap();
        assert_eq!(projection.data, json!({"values": ["third"]}));
    }

    #[test]
    fn scoped_denial_removes_only_the_bound_record() {
        let default = PolicyDocument {
            policy_ref: "policy:private-default".to_owned(),
            version: 1,
            default_effect: Effect::Allow,
            rules: vec![],
        };
        let scoped = ScopedPolicy {
            path: "/items/0".to_owned(),
            reason: "record_policy".to_owned(),
            policy: PolicyDocument {
                policy_ref: "policy:withheld-record".to_owned(),
                version: 4,
                default_effect: Effect::Deny,
                rules: vec![],
            },
        };
        let projection = project_scoped(
            json!({"items": [{"secret": "hidden"}, {"public": "kept"}]}),
            &[default],
            &[scoped],
            "principal:self",
            "task_reasoning",
            "read",
        )
        .unwrap();
        assert_eq!(projection.data, json!({"items": [{"public": "kept"}]}));
        assert_eq!(projection.receipt.withheld[0].path, "/items/0");
        assert_eq!(projection.receipt.withheld[0].reason, "record_policy");
    }

    #[test]
    fn output_hash_is_over_projected_data_before_a_receipt_is_attached() {
        let policy = PolicyDocument {
            policy_ref: "policy:allow".to_owned(),
            version: 1,
            default_effect: Effect::Allow,
            rules: vec![],
        };
        let projection = project(
            json!({"value": 42}),
            &[policy],
            "principal:self",
            "task_reasoning",
            "read",
        )
        .unwrap();
        let expected = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(
                serde_json::to_vec(&projection.data).unwrap()
            ))
        );
        assert_eq!(projection.receipt.output_hash, expected);

        let mut with_receipt = projection.data;
        with_receipt
            .as_object_mut()
            .unwrap()
            .insert("projection".to_owned(), json!({"output_hash": expected}));
        let attached_hash = format!(
            "sha256:{}",
            hex::encode(Sha256::digest(serde_json::to_vec(&with_receipt).unwrap()))
        );
        assert_ne!(attached_hash, expected);
    }
}
