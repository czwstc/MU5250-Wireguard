fn main() {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let adb =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets/platform-tools/adb");
        if let Ok(metadata) = std::fs::metadata(&adb) {
            let mut permissions = metadata.permissions();
            permissions.set_mode(permissions.mode() | 0o755);
            std::fs::set_permissions(&adb, permissions)
                .expect("failed to mark bundled adb executable");
        }
        println!("cargo:rerun-if-changed={}", adb.display());
    }
    tauri_build::build()
}
