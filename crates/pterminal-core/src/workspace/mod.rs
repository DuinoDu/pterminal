use crate::split::{PaneId, SplitTree};

pub type WorkspaceId = u64;

#[derive(Debug)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub split_tree: SplitTree,
    active_pane: PaneId,
}

impl Workspace {
    pub fn new(id: WorkspaceId, pane_id: PaneId) -> Self {
        Self {
            id,
            name: format!("Workspace {}", id),
            split_tree: SplitTree::new(pane_id),
            active_pane: pane_id,
        }
    }

    pub fn active_pane(&self) -> PaneId {
        self.active_pane
    }

    pub fn set_active_pane(&mut self, id: PaneId) {
        if self.split_tree.contains(id) {
            self.active_pane = id;
        }
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.split_tree.pane_ids()
    }
}

#[derive(Debug)]
pub struct WorkspaceManager {
    workspaces: Vec<Workspace>,
    active_index: usize,
    next_workspace_id: WorkspaceId,
    next_pane_id: PaneId,
}

impl WorkspaceManager {
    pub fn new() -> Self {
        let ws = Workspace::new(0, 0);
        Self {
            workspaces: vec![ws],
            active_index: 0,
            next_workspace_id: 1,
            next_pane_id: 1,
        }
    }

    pub fn add_workspace(&mut self) -> (WorkspaceId, PaneId) {
        let ws_id = self.next_workspace_id;
        let pane_id = self.next_pane_id;
        self.next_workspace_id += 1;
        self.next_pane_id += 1;
        let ws = Workspace::new(ws_id, pane_id);
        self.workspaces.push(ws);
        self.active_index = self.workspaces.len() - 1;
        (ws_id, pane_id)
    }

    pub fn close_workspace(&mut self, id: WorkspaceId) {
        if self.workspaces.len() <= 1 {
            return; // don't close the last workspace
        }
        if let Some(pos) = self.workspaces.iter().position(|ws| ws.id == id) {
            self.workspaces.remove(pos);
            if self.active_index >= self.workspaces.len() {
                self.active_index = self.workspaces.len() - 1;
            }
        }
    }

    pub fn select_workspace(&mut self, idx: usize) {
        if idx < self.workspaces.len() {
            self.active_index = idx;
        }
    }

    pub fn active_workspace(&self) -> &Workspace {
        &self.workspaces[self.active_index]
    }

    pub fn active_workspace_mut(&mut self) -> &mut Workspace {
        &mut self.workspaces[self.active_index]
    }

    pub fn workspace_count(&self) -> usize {
        self.workspaces.len()
    }

    pub fn active_index(&self) -> usize {
        self.active_index
    }

    pub fn workspaces(&self) -> &[Workspace] {
        &self.workspaces
    }

    /// Allocate a new pane ID (used when splitting panes).
    pub fn next_pane_id(&mut self) -> PaneId {
        let id = self.next_pane_id;
        self.next_pane_id += 1;
        id
    }
}

impl Default for WorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_manager_has_one_workspace() {
        let mgr = WorkspaceManager::new();
        assert_eq!(mgr.workspace_count(), 1);
        assert_eq!(mgr.active_index(), 0);
        assert_eq!(mgr.active_workspace().pane_ids(), vec![0]);
    }

    #[test]
    fn add_and_select_workspace() {
        let mut mgr = WorkspaceManager::new();
        let (ws_id, pane_id) = mgr.add_workspace();
        assert_eq!(mgr.workspace_count(), 2);
        assert_eq!(mgr.active_index(), 1);
        assert_eq!(ws_id, 1);
        assert_eq!(pane_id, 1);

        mgr.select_workspace(0);
        assert_eq!(mgr.active_index(), 0);
    }

    #[test]
    fn close_workspace() {
        let mut mgr = WorkspaceManager::new();
        mgr.add_workspace();
        assert_eq!(mgr.workspace_count(), 2);
        mgr.close_workspace(1);
        assert_eq!(mgr.workspace_count(), 1);
    }

    #[test]
    fn cannot_close_last_workspace() {
        let mut mgr = WorkspaceManager::new();
        mgr.close_workspace(0);
        assert_eq!(mgr.workspace_count(), 1);
    }
}
