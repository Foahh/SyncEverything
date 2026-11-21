use std::path::PathBuf;

fn main() {
    let out = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("../src/bindings.ts"));
    relay_desktop_lib::export_bindings_to(&out)
        .expect("failed to export tauri-specta TypeScript bindings");
    println!("Exported TypeScript bindings to {}", out.display());
}
