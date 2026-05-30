// Suppress the default Windows console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! Desktop binary entry point. Delegates to [`rs_nessie_lib::run`].

fn main() {
    rs_nessie_lib::run();
}
