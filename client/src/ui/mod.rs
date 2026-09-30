//! The egui application shell (§19, §20, §30).

pub mod app;
pub mod screens;
pub mod theme;
pub mod widgets;

pub use app::{run, GameBridgeApp, Screen};
