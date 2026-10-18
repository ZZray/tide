fn main() {
    #[cfg(windows)]
    {
        println!("cargo:rerun-if-changed=../../assets/brand/tide.ico");
        winresource::WindowsResource::new()
            .set_icon("../../assets/brand/tide.ico")
            .set("ProductName", "Tide")
            .set("FileDescription", "Tide - Git document sync")
            .set(
                "LegalCopyright",
                "Copyright (c) 2026 Tide contributors. MIT License.",
            )
            .compile()
            .expect("embed Tide Windows resources");
    }
    #[cfg(feature = "desktop")]
    slint_build::compile_with_config(
        "../../ui/main.slint",
        slint_build::CompilerConfiguration::new().with_debug_info(true),
    )
    .expect("compile Tide UI");
}
