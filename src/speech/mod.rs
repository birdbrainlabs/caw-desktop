//! Speech-to-text pieces that need no model or microphone: cutting audio at
//! pauses and tidying the recognizer's text. Used by `caw-desktop dictate`.

pub mod cleaner;
pub mod filter;
pub mod segmenter;
pub mod text;
