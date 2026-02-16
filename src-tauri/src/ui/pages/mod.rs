pub mod general;
pub mod models;
pub mod advanced;
pub mod history;
pub mod post_process;
pub mod debug;
pub mod about;

use gtk::Widget;

pub trait Page {
    fn widget(&self) -> &Widget;
}
