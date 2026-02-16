use gtk::prelude::*;
use gtk::{Box, Orientation};
use libadwaita::{Application as AdwApplication, HeaderBar, Window, WindowTitle};
use std::sync::Arc;

use crate::app::AppState;
use super::sidebar::Sidebar;
use super::pages::Page;

pub struct MainWindow {
    window: Window,
}

impl MainWindow {
    pub fn new(app: &AdwApplication, state: Arc<AppState>) -> Self {
        let window = Window::builder()
            .application(app)
            .title("Handy")
            .default_width(900)
            .default_height(650)
            .build();

        let main_box = Box::builder()
            .orientation(Orientation::Horizontal)
            .build();

        let sidebar = Sidebar::new(&state);
        
        let content_box = Box::builder()
            .orientation(Orientation::Vertical)
            .hexpand(true)
            .build();

        let header = HeaderBar::builder()
            .title_widget(&WindowTitle::new("Handy", "General"))
            .build();
        content_box.append(&header);

        let stack = gtk::Stack::builder()
            .hexpand(true)
            .vexpand(true)
            .build();

        let general_page = super::pages::general::GeneralPage::new(&state);
        stack.add_titled(&general_page.widget(), Some("general"), "General");

        let models_page = super::pages::models::ModelsPage::new(&state);
        stack.add_titled(&models_page.widget(), Some("models"), "Models");

        let advanced_page = super::pages::advanced::AdvancedPage::new(&state);
        stack.add_titled(&advanced_page.widget(), Some("advanced"), "Advanced");

        let history_page = super::pages::history::HistoryPage::new(&state);
        stack.add_titled(&history_page.widget(), Some("history"), "History");

        if state.settings.post_process_enabled() || state.settings.debug_mode() {
            let post_process_page = super::pages::post_process::PostProcessPage::new(&state);
            stack.add_titled(&post_process_page.widget(), Some("post-process"), "Post-Processing");
        }

        if state.settings.debug_mode() {
            let debug_page = super::pages::debug::DebugPage::new(&state);
            stack.add_titled(&debug_page.widget(), Some("debug"), "Debug");
        }

        let about_page = super::pages::about::AboutPage::new();
        stack.add_titled(&about_page.widget(), Some("about"), "About");

        content_box.append(&stack);

        sidebar.connect_stack(&stack);

        main_box.append(&sidebar.widget());
        main_box.append(&content_box);

        window.set_content(Some(&main_box));

        Self { window }
    }

    pub fn present(&self) {
        self.window.present();
    }

    pub fn widget(&self) -> &Window {
        &self.window
    }
}
