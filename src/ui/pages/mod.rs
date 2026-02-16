pub mod general;
pub mod models;
pub mod advanced;
pub mod about;

use gtk::Widget;

pub trait Page {
    fn widget(&self) -> &Widget;
}
