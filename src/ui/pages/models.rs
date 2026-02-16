use gtk::prelude::*;
use gtk::{Widget, ScrolledWindow};
use std::sync::Arc;

use crate::app::AppState;
use super::Page;

pub struct ModelsPage {
    container: ScrolledWindow,
}

impl ModelsPage {
    pub fn new(_state: &Arc<AppState>) -> Self {
        let container = ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .build();
        Self { container }
    }
}

impl Page for ModelsPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}
