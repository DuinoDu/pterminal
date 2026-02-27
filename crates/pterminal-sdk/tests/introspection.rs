use pterminal_plugin_api::{
    PaneContentSnapshot, PaneStateSnapshot, TerminalTopology, WorkspaceTopology,
};
use pterminal_sdk::{TerminalIntrospectionApi, TerminalSnapshotProvider};

#[derive(Default)]
struct MockTerminalProvider;

impl TerminalSnapshotProvider for MockTerminalProvider {
    fn topology(&self) -> anyhow::Result<TerminalTopology> {
        Ok(TerminalTopology {
            workspaces: vec![WorkspaceTopology {
                id: 1,
                name: "Main".into(),
                pane_ids: vec![10],
                active_pane_id: 10,
            }],
        })
    }

    fn pane_states(&self) -> anyhow::Result<Vec<PaneStateSnapshot>> {
        Ok(vec![PaneStateSnapshot {
            pane_id: 10,
            alive: true,
            title: "shell".into(),
            cwd: "/tmp".into(),
            rows: 24,
            cols: 80,
            focused: true,
        }])
    }

    fn pane_content(&self, pane_id: u64, max_lines: usize) -> anyhow::Result<PaneContentSnapshot> {
        Ok(PaneContentSnapshot {
            pane_id,
            text: format!("content:{max_lines}"),
            truncated: false,
        })
    }
}

#[test]
fn topology_read_requires_permission() {
    let mut api = TerminalIntrospectionApi::new(
        MockTerminalProvider,
        vec![],
        3,
    );
    let err = api.topology().expect_err("permission should be required");
    assert!(err.to_string().contains("terminal.topology.read"));

    let mut api = TerminalIntrospectionApi::new(
        MockTerminalProvider,
        vec!["terminal.topology.read".into()],
        3,
    );
    let topology = api.topology().expect("topology read");
    assert_eq!(topology.workspaces.len(), 1);
}

#[test]
fn pane_content_enforces_rate_limit() {
    let mut api = TerminalIntrospectionApi::new(
        MockTerminalProvider,
        vec!["terminal.pane.content.read".into()],
        2,
    );
    api.pane_content(10, 10).expect("first read");
    api.pane_content(10, 10).expect("second read");
    let err = api.pane_content(10, 10).expect_err("third read should fail");
    assert!(err.to_string().contains("rate limit"));
}
