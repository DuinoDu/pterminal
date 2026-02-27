use pterminal_plugin_api::SidebarViewContribution;
use pterminal_ui::plugin::ContributionRegistry;

#[test]
fn registry_orders_sidebar_views_and_marks_active_item() {
    let mut registry = ContributionRegistry::new();
    registry.replace_sidebar_views(vec![
        SidebarViewContribution {
            id: "b".into(),
            title: "B".into(),
            icon: None,
            order: 200,
        },
        SidebarViewContribution {
            id: "a".into(),
            title: "A".into(),
            icon: None,
            order: 100,
        },
    ]);
    registry.set_active_sidebar("a");

    let items = registry.sidebar_items();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].view_id, "a");
    assert!(items[0].active);
}

#[test]
fn registry_maps_builtin_sidebar_to_workspace_indexes() {
    let mut registry = ContributionRegistry::new();
    registry.set_builtin_workspace_sidebar(3, 1);

    assert_eq!(registry.sidebar_id_at(0), Some("builtin.workspace.0"));
    assert_eq!(registry.sidebar_id_at(1), Some("builtin.workspace.1"));
    assert_eq!(
        ContributionRegistry::builtin_workspace_index("builtin.workspace.2"),
        Some(2)
    );
}
