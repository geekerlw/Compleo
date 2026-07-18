use std::process::Command;

fn main() {
    tauri_build::build();

    #[cfg(target_os = "macos")]
    {
        println!("cargo:rustc-link-lib=framework=ImageIO");

        // Compile Swift OCR helper
        let swift_src = std::path::Path::new("swift-ocr/main.swift");
        let swift_out = std::path::Path::new("swift-ocr/compleo-ocr");

        // Only recompile if source is newer than output
        let should_compile = if swift_out.exists() {
            let src_modified = std::fs::metadata(swift_src)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            let out_modified = std::fs::metadata(swift_out)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
            src_modified > out_modified
        } else {
            true
        };

        if should_compile {
            let status = Command::new("swiftc")
                .args([
                    "-O",
                    "-o",
                    swift_out.to_str().unwrap(),
                    swift_src.to_str().unwrap(),
                    "-framework", "Vision",
                    "-framework", "AppKit",
                    "-framework", "CoreGraphics",
                    "-framework", "ImageIO",
                ])
                .status()
                .expect("Failed to compile Swift OCR helper");

            if !status.success() {
                panic!("Swift OCR compilation failed");
            }
        }

        println!("cargo:rerun-if-changed=swift-ocr/main.swift");
    }
}
