use std::collections::HashSet;

use crate::model::{
    AccessibilityAction, AccessibilityElementId, AccessibilityNode, AccessibilityRect,
};

pub const MAX_DEPTH: usize = 16;
pub const MAX_NODES: usize = 512;
pub const MAX_STRING_CHARS: usize = 256;
pub const MAX_VALUE_CHARS: usize = 256;
const MAX_CHILDREN: usize = 128;

#[derive(Clone, Debug, PartialEq)]
pub struct RawNode {
    pub key: u64,
    pub role: Option<String>,
    pub subrole: Option<String>,
    pub title: Option<String>,
    pub value: Option<String>,
    pub bounds: Option<AccessibilityRect>,
    pub enabled: Option<bool>,
    pub focused: Option<bool>,
    pub secure: bool,
    pub supported_actions: Vec<AccessibilityAction>,
    pub value_settable: bool,
    pub children: Vec<Self>,
    pub truncated: bool,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct NormalizedTree {
    pub root: Option<AccessibilityNode>,
    pub nodes: usize,
}

pub fn normalize_tree(raw: &RawNode, generation: u64) -> NormalizedTree {
    let mut context = NormalizeContext {
        generation,
        nodes: 0,
        next_id: 0,
        visited: HashSet::new(),
    };
    let root = context.node(raw, 0);
    NormalizedTree {
        root,
        nodes: context.nodes,
    }
}

struct NormalizeContext {
    generation: u64,
    nodes: usize,
    next_id: usize,
    visited: HashSet<u64>,
}

impl NormalizeContext {
    fn node(&mut self, raw: &RawNode, depth: usize) -> Option<AccessibilityNode> {
        if depth > MAX_DEPTH || self.nodes >= MAX_NODES || !self.visited.insert(raw.key) {
            return None;
        }
        self.nodes += 1;
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let mut children = Vec::new();
        let mut truncated = raw.truncated;
        for child in raw.children.iter().take(MAX_CHILDREN) {
            if let Some(node) = self.node(child, depth + 1) {
                children.push(node);
            } else {
                truncated = true;
                break;
            }
        }
        if raw.children.len() > MAX_CHILDREN {
            truncated = true;
        }
        let value = if raw.secure || raw.role.as_deref() == Some("AXSecureTextField") {
            raw.value.as_ref().map(|_| "[redacted]".to_owned())
        } else {
            sanitize(raw.value.as_deref(), MAX_VALUE_CHARS)
        };
        Some(AccessibilityNode {
            element: Some(AccessibilityElementId {
                id: format!("e{id}"),
                generation: self.generation,
            }),
            role: sanitize(raw.role.as_deref(), MAX_STRING_CHARS),
            subrole: sanitize(raw.subrole.as_deref(), MAX_STRING_CHARS),
            title: sanitize(raw.title.as_deref(), MAX_STRING_CHARS),
            value,
            bounds: raw.bounds,
            enabled: raw.enabled,
            focused: raw.focused,
            children_count: raw.children.len(),
            children,
            truncated,
            supported_actions: raw.supported_actions.clone(),
            value_settable: raw.value_settable,
        })
    }
}

pub(crate) fn sanitize(value: Option<&str>, limit: usize) -> Option<String> {
    value.map(|value| {
        value
            .chars()
            .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
            .map(|character| if character == '\n' { ' ' } else { character })
            .take(limit)
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::{MAX_DEPTH, MAX_NODES, RawNode, normalize_tree};
    use crate::{AccessibilityAction, AccessibilityRect};

    fn node(key: u64) -> RawNode {
        RawNode {
            key,
            role: Some("AXButton".into()),
            subrole: None,
            title: Some("Launch".into()),
            value: Some("secret".into()),
            bounds: Some(AccessibilityRect {
                x: 1.0,
                y: 2.0,
                width: 3.0,
                height: 4.0,
            }),
            enabled: Some(true),
            focused: Some(false),
            secure: false,
            supported_actions: vec![AccessibilityAction::Press],
            value_settable: false,
            children: Vec::new(),
            truncated: false,
        }
    }

    #[test]
    fn normalizes_owned_bounded_tree_and_tokens() {
        let tree = normalize_tree(&node(1), 7);
        let root = tree.root.unwrap();
        assert_eq!(root.element.unwrap().generation, 7);
        assert_eq!(root.title.as_deref(), Some("Launch"));
        assert_eq!(root.children_count, 0);
    }

    #[test]
    fn redacts_secure_values_and_breaks_cycles() {
        let mut root = node(1);
        root.role = Some("AXSecureTextField".into());
        root.children.push(node(1));
        let tree = normalize_tree(&root, 1);
        let root = tree.root.unwrap();
        assert_eq!(root.value.as_deref(), Some("[redacted]"));
        assert!(root.children.is_empty());
        assert!(root.truncated);
    }

    #[test]
    fn bounds_depth_and_node_count() {
        let mut root = node(0);
        let mut cursor = &mut root;
        for key in 1..=(MAX_DEPTH as u64 + 2) {
            cursor.children.push(node(key));
            cursor = cursor.children.last_mut().unwrap();
        }
        let tree = normalize_tree(&root, 2);
        assert!(tree.nodes <= MAX_DEPTH + 1);

        let mut wide = node(10);
        wide.children = (11..(11 + MAX_NODES as u64 + 10)).map(node).collect();
        let tree = normalize_tree(&wide, 3);
        assert!(tree.nodes <= MAX_NODES);
        assert!(tree.root.unwrap().truncated);
    }

    #[test]
    fn actionable_tokens_are_unique() {
        let mut root = node(1);
        root.children.push(node(2));
        root.children.push(node(3));
        let tree = normalize_tree(&root, 8).root.unwrap();
        let ids = [
            tree.element.as_ref().unwrap().id.clone(),
            tree.children[0].element.as_ref().unwrap().id.clone(),
            tree.children[1].element.as_ref().unwrap().id.clone(),
        ];
        assert_eq!(
            ids.len(),
            ids.iter().collect::<std::collections::HashSet<_>>().len()
        );
    }
}
