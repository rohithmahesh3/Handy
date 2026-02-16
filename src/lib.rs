pub mod actions;
pub mod app;
pub mod audio_feedback;
pub mod audio_toolkit;
pub mod dbus;
pub mod ibus_engine;
pub mod llm_client;
pub mod managers;
pub mod settings;
pub mod text_utils;
pub mod ui;

use std::sync::atomic::AtomicU8;

pub static FILE_LOG_LEVEL: AtomicU8 = AtomicU8::new(log::LevelFilter::Debug as u8);

pub fn level_filter_from_u8(value: u8) -> log::LevelFilter {
    match value {
        0 => log::LevelFilter::Off,
        1 => log::LevelFilter::Error,
        2 => log::LevelFilter::Warn,
        3 => log::LevelFilter::Info,
        4 => log::LevelFilter::Debug,
        5 => log::LevelFilter::Trace,
        _ => log::LevelFilter::Trace,
    }
}
