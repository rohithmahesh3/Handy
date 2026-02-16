use gtk::prelude::*;
use gtk::{Widget, ScrolledWindow};
use std::sync::Arc;

use crate::app::AppState;
use super::Page;

pub struct DebugPage {
    container: ScrolledWindow,
}

impl DebugPage {
    pub fn new(_state: &Arc<AppState>) -> Self {
        let container = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        Self { container }
    }
}

impl Page for DebugPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}
