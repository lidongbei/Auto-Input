#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod hotkey;
mod input;

fn main() -> iced::Result {
    app::run()
}
