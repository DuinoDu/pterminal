pub type PaneId = u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SplitDirection {
    Horizontal, // left | right
    Vertical,   // top / bottom
}

#[derive(Debug, Clone)]
pub struct PaneRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

#[derive(Debug)]
pub struct SplitTree {
    root: SplitNode,
}

#[derive(Debug)]
enum SplitNode {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<SplitNode>,
        second: Box<SplitNode>,
    },
}

impl SplitTree {
    pub fn new(pane_id: PaneId) -> Self {
        Self {
            root: SplitNode::Leaf(pane_id),
        }
    }

    pub fn split(&mut self, target: PaneId, direction: SplitDirection, new_pane: PaneId) {
        Self::split_node(&mut self.root, target, direction, new_pane);
    }

    fn split_node(
        node: &mut SplitNode,
        target: PaneId,
        direction: SplitDirection,
        new_pane: PaneId,
    ) -> bool {
        match node {
            SplitNode::Leaf(id) if *id == target => {
                let old = std::mem::replace(node, SplitNode::Leaf(0));
                *node = SplitNode::Split {
                    direction,
                    ratio: 0.5,
                    first: Box::new(old),
                    second: Box::new(SplitNode::Leaf(new_pane)),
                };
                true
            }
            SplitNode::Leaf(_) => false,
            SplitNode::Split { first, second, .. } => {
                Self::split_node(first, target, direction, new_pane)
                    || Self::split_node(second, target, direction, new_pane)
            }
        }
    }

    /// Removes a pane, promoting its sibling. Returns false if it's the only pane.
    pub fn remove(&mut self, pane_id: PaneId) -> bool {
        if let SplitNode::Leaf(id) = &self.root {
            if *id == pane_id {
                return false; // can't remove the only pane
            }
        }
        Self::remove_node(&mut self.root, pane_id)
    }

    fn remove_node(node: &mut SplitNode, pane_id: PaneId) -> bool {
        match node {
            SplitNode::Leaf(_) => false,
            SplitNode::Split { first, second, .. } => {
                // Check if first child is the target leaf
                if let SplitNode::Leaf(id) = first.as_ref() {
                    if *id == pane_id {
                        let sibling = std::mem::replace(second.as_mut(), SplitNode::Leaf(0));
                        *node = sibling;
                        return true;
                    }
                }
                // Check if second child is the target leaf
                if let SplitNode::Leaf(id) = second.as_ref() {
                    if *id == pane_id {
                        let sibling = std::mem::replace(first.as_mut(), SplitNode::Leaf(0));
                        *node = sibling;
                        return true;
                    }
                }
                // Recurse
                Self::remove_node(first, pane_id) || Self::remove_node(second, pane_id)
            }
        }
    }

    pub fn layout(&self) -> Vec<(PaneId, PaneRect)> {
        let mut result = Vec::new();
        Self::layout_node(
            &self.root,
            &PaneRect {
                x: 0.0,
                y: 0.0,
                width: 1.0,
                height: 1.0,
            },
            &mut result,
        );
        result
    }

    fn layout_node(node: &SplitNode, rect: &PaneRect, out: &mut Vec<(PaneId, PaneRect)>) {
        match node {
            SplitNode::Leaf(id) => {
                out.push((*id, rect.clone()));
            }
            SplitNode::Split {
                direction,
                ratio,
                first,
                second,
            } => match direction {
                SplitDirection::Horizontal => {
                    let first_w = rect.width * ratio;
                    let first_rect = PaneRect {
                        x: rect.x,
                        y: rect.y,
                        width: first_w,
                        height: rect.height,
                    };
                    let second_rect = PaneRect {
                        x: rect.x + first_w,
                        y: rect.y,
                        width: rect.width - first_w,
                        height: rect.height,
                    };
                    Self::layout_node(first, &first_rect, out);
                    Self::layout_node(second, &second_rect, out);
                }
                SplitDirection::Vertical => {
                    let first_h = rect.height * ratio;
                    let first_rect = PaneRect {
                        x: rect.x,
                        y: rect.y,
                        width: rect.width,
                        height: first_h,
                    };
                    let second_rect = PaneRect {
                        x: rect.x,
                        y: rect.y + first_h,
                        width: rect.width,
                        height: rect.height - first_h,
                    };
                    Self::layout_node(first, &first_rect, out);
                    Self::layout_node(second, &second_rect, out);
                }
            },
        }
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        let mut ids = Vec::new();
        Self::collect_ids(&self.root, &mut ids);
        ids
    }

    fn collect_ids(node: &SplitNode, out: &mut Vec<PaneId>) {
        match node {
            SplitNode::Leaf(id) => out.push(*id),
            SplitNode::Split { first, second, .. } => {
                Self::collect_ids(first, out);
                Self::collect_ids(second, out);
            }
        }
    }

    pub fn contains(&self, pane_id: PaneId) -> bool {
        Self::node_contains(&self.root, pane_id)
    }

    fn node_contains(node: &SplitNode, pane_id: PaneId) -> bool {
        match node {
            SplitNode::Leaf(id) => *id == pane_id,
            SplitNode::Split { first, second, .. } => {
                Self::node_contains(first, pane_id) || Self::node_contains(second, pane_id)
            }
        }
    }

    pub fn next_pane(&self, current: PaneId) -> Option<PaneId> {
        let ids = self.pane_ids();
        let pos = ids.iter().position(|&id| id == current)?;
        Some(ids[(pos + 1) % ids.len()])
    }

    pub fn prev_pane(&self, current: PaneId) -> Option<PaneId> {
        let ids = self.pane_ids();
        let pos = ids.iter().position(|&id| id == current)?;
        Some(ids[(pos + ids.len() - 1) % ids.len()])
    }

    /// Adjust the ratio of the parent split containing `pane_id` by `delta`.
    pub fn adjust_ratio(&mut self, pane_id: PaneId, delta: f32) {
        Self::adjust_ratio_node(&mut self.root, pane_id, delta);
    }

    fn adjust_ratio_node(node: &mut SplitNode, pane_id: PaneId, delta: f32) -> bool {
        match node {
            SplitNode::Leaf(_) => false,
            SplitNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
                // Check if either direct child is the target
                let first_match = Self::node_contains(first, pane_id);
                let second_match = Self::node_contains(second, pane_id);
                if first_match || second_match {
                    // Only adjust if one of our direct children contains it
                    // but try recursing first to find the closest parent
                    let recursed = if first_match {
                        Self::adjust_ratio_node(first, pane_id, delta)
                    } else {
                        Self::adjust_ratio_node(second, pane_id, delta)
                    };
                    if !recursed {
                        // We are the closest parent split
                        *ratio = (*ratio + delta).clamp(0.1, 0.9);
                        return true;
                    }
                    return recursed;
                }
                false
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_pane_layout() {
        let tree = SplitTree::new(1);
        let layout = tree.layout();
        assert_eq!(layout.len(), 1);
        assert_eq!(layout[0].0, 1);
        let r = &layout[0].1;
        assert!((r.x - 0.0).abs() < f32::EPSILON);
        assert!((r.width - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn horizontal_split() {
        let mut tree = SplitTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        let layout = tree.layout();
        assert_eq!(layout.len(), 2);
        assert!((layout[0].1.width - 0.5).abs() < f32::EPSILON);
        assert!((layout[1].1.x - 0.5).abs() < f32::EPSILON);
    }

    #[test]
    fn remove_pane() {
        let mut tree = SplitTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        assert!(tree.remove(2));
        assert_eq!(tree.pane_ids(), vec![1]);
    }

    #[test]
    fn cannot_remove_only_pane() {
        let mut tree = SplitTree::new(1);
        assert!(!tree.remove(1));
    }

    #[test]
    fn next_prev_pane() {
        let mut tree = SplitTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        tree.split(2, SplitDirection::Vertical, 3);
        assert_eq!(tree.next_pane(1), Some(2));
        assert_eq!(tree.next_pane(3), Some(1)); // wraps
        assert_eq!(tree.prev_pane(1), Some(3)); // wraps
    }

    #[test]
    fn adjust_ratio() {
        let mut tree = SplitTree::new(1);
        tree.split(1, SplitDirection::Horizontal, 2);
        tree.adjust_ratio(1, 0.1);
        let layout = tree.layout();
        assert!((layout[0].1.width - 0.6).abs() < f32::EPSILON);
    }
}
