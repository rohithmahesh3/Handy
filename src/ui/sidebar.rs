use gtk::prelude::*;
use gtk::{ListBox, ListBoxRow, Box, Orientation, Image, Label};
use std::sync::Arc;

use crate::app::AppState;

pub struct Sidebar {
    list: ListBox,
}

impl Sidebar {
    pub fn new(state: &Arc<AppState>) -> Self {
        let list = ListBox::builder()
            .css_classes(["navigation-sidebar"])
            .build();

        add_item(&list, "general", "General", "preferences-system-symbolic");
        add_item(&list, "models", "Models", "folder-download-symbolic");
        add_item(&list, "advanced", "Advanced", "applications-engineering-symbolic");

        if state.settings.post_process_enabled() || state.settings.debug_mode() {
            add_item(&list, "post-process", "Post-Processing", "text-editor-symbolic");
        }

        add_item(&list, "about", "About", "help-about-symbolic");

        Self { list }
    }

    pub fn widget(&self) -> &ListBox {
        &self.list
    }

    pub fn connect_stack(&self, stack: &gtk::Stack) {
        let stack = stack.clone();
        self.list.connect_row_selected(move |_, row| {
            if let Some(row) = row {
                if let Some(name) = row.widget_name().as_str().split("::").last() {
                    stack.set_visible_child_name(name);
                }
            }
        });
    }
}

fn add_item(list: &ListBox, name: &str, label: &str, icon: &str) {
    let row = ListBoxRow::builder()
        .name(format!("sidebar::{}", name))
        .selectable(true)
        .activatable(true)
        .build();

    let box_ = Box::builder()
        .orientation(Orientation::Horizontal)
        .spacing(12)
        .margin_top(8)
        .margin_bottom(8)
        .margin_start(12)
        .margin_end(12)
        .build();

    let icon = Image::from_icon_name(icon);
    box_.append(&icon);

    let label = Label::builder()
        .label(label)
        .xalign(0.0)
        .build();
    box_.append(&label);

    row.set_child(Some(&box_));
    list.append(&row);
}
