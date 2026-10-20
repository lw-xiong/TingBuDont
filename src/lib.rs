//! Library facade: re-exports pipeline modules so examples/tests/integration
//! tests can drive them directly. The binary (`main.rs`) uses the same modules.

pub mod cli;
pub mod extract;
pub mod layout;
pub mod markdown;
pub mod ocr;
pub mod output;
pub mod types;
