use pterminal_plugin_api::SidebarViewContribution;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrySidebarItem {
    pub view_id: String,
    pub title: String,
    pub active: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ContributionRegistry {
    sidebar_views: Vec<SidebarViewContribution>,
    active_sidebar_view: Option<String>,
}

impl ContributionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn replace_sidebar_views(&mut self, mut views: Vec<SidebarViewContribution>) {
        views.sort_by(|a, b| a.order.cmp(&b.order).then_with(|| a.title.cmp(&b.title)));
        self.sidebar_views = views;
    }

    pub fn set_active_sidebar(&mut self, view_id: impl Into<String>) {
        self.active_sidebar_view = Some(view_id.into());
    }

    pub fn sidebar_items(&self) -> Vec<RegistrySidebarItem> {
        self.sidebar_views
            .iter()
            .map(|view| RegistrySidebarItem {
                view_id: view.id.clone(),
                title: view.title.clone(),
                active: self.active_sidebar_view.as_deref() == Some(view.id.as_str()),
            })
            .collect()
    }

    pub fn sidebar_id_at(&self, idx: usize) -> Option<&str> {
        self.sidebar_views.get(idx).map(|v| v.id.as_str())
    }

    pub fn set_builtin_workspace_sidebar(&mut self, workspace_count: usize, active_idx: usize) {
        let views = (0..workspace_count)
            .map(|idx| SidebarViewContribution {
                id: format!("builtin.workspace.{idx}"),
                title: format!("Workspace {}", idx + 1),
                icon: None,
                order: idx as i32,
            })
            .collect();
        self.replace_sidebar_views(views);
        self.set_active_sidebar(format!("builtin.workspace.{active_idx}"));
    }

    pub fn builtin_workspace_index(view_id: &str) -> Option<usize> {
        view_id.strip_prefix("builtin.workspace.")?.parse().ok()
    }
}
