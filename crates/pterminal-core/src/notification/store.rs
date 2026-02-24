use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub id: u64,
    pub title: String,
    pub body: String,
    pub created_at_ms: u128,
    pub read: bool,
}

#[derive(Debug, Default, Clone)]
pub struct NotificationStore {
    next_id: u64,
    items: Vec<Notification>,
}

impl NotificationStore {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            items: Vec::new(),
        }
    }

    pub fn push(&mut self, title: impl Into<String>, body: impl Into<String>) -> Notification {
        let notification = Notification {
            id: self.next_id,
            title: title.into(),
            body: body.into(),
            created_at_ms: now_ms(),
            read: false,
        };
        self.next_id += 1;
        self.items.push(notification.clone());
        notification
    }

    pub fn list(&self) -> &[Notification] {
        &self.items
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }

    pub fn mark_all_read(&mut self) {
        for item in &mut self.items {
            item.read = true;
        }
    }

    pub fn unread_count(&self) -> usize {
        self.items.iter().filter(|n| !n.read).count()
    }
}

fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}
