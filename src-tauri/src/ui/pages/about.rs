use gtk::prelude::*;
use gtk::{Widget, Box, Orientation, Label, Image};
use libadwaita::Clamp;
use super::Page;

const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub struct AboutPage {
    container: Clamp,
}

impl AboutPage {
    pub fn new() -> Self {
        let container = Clamp::builder()
            .maximum_size(600)
            .build();

        let vbox = Box::builder()
            .orientation(Orientation::Vertical)
            .spacing(24)
            .margin_top(48)
            .margin_bottom(24)
            .halign(gtk::Align::Center)
            .build();

        let logo = Image::builder()
            .icon_name("handy")
            .pixel_size(128)
            .build();
        vbox.append(&logo);

        let name = Label::builder()
            .label("Handy")
            .css_classes(["title-1"])
            .build();
        vbox.append(&name);

        let version = Label::builder()
            .label(format!("Version {}", APP_VERSION))
            .css_classes(["dim-label"])
            .build();
        vbox.append(&version);

        let description = Label::builder()
            .label("Speech-to-text for GNOME/Wayland via IBus")
            .wrap(true)
            .justify(gtk::Justification::Center)
            .build();
        vbox.append(&description);

        let links = Box::builder()
            .orientation(Orientation::Horizontal)
            .spacing(12)
            .halign(gtk::Align::Center)
            .margin_top(24)
            .build();

        let website_btn = gtk::Button::builder()
            .label("Website")
            .css_classes(["pill", "suggested-action"])
            .build();
        website_btn.connect_clicked(|_| {
            let _ = gtk::UriLauncher::new("https://github.com/rohithmahesh/Handy")
                .launch(None::<&gtk::Window>, None::<gio::Cancellable>, |_| {});
        });
        links.append(&website_btn);

        let issue_btn = gtk::Button::builder()
            .label("Report Issue")
            .css_classes(["pill"])
            .build();
        issue_btn.connect_clicked(|_| {
            let _ = gtk::UriLauncher::new("https://github.com/rohithmahesh/Handy/issues")
                .launch(None::<&gtk::Window>, None::<gio::Cancellable>, |_| {});
        });
        links.append(&issue_btn);

        vbox.append(&links);

        let license_label = Label::builder()
            .label("Licensed under MIT")
            .css_classes(["dim-label", "caption"])
            .margin_top(24)
            .build();
        vbox.append(&license_label);

        container.set_child(Some(&vbox));

        Self { container }
    }
}

impl Page for AboutPage {
    fn widget(&self) -> &Widget {
        self.container.upcast_ref()
    }
}
