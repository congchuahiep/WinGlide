fn main() {
    windows_reactor_setup::as_self_contained();

    // Embed the application icon and VERSIONINFO block. `as_self_contained`
    // already links the SxS manifest through `/MANIFESTINPUT`, and Windows
    // rejects a second manifest resource, so this resource script carries only
    // the icon and version info.
    let mut resource = winresource::WindowsResource::new();
    resource.set_icon("assets/app.ico");
    resource.set_language(0x0409);
    resource.set("ProductName", "WinGlide");
    resource.set("FileDescription", "WinGlide");
    resource.set("CompanyName", "congchuahiep");
    resource.set("LegalCopyright", "MIT License");
    resource.set("OriginalFilename", "WinGlide.exe");
    resource
        .compile()
        .expect("failed to compile Windows resources");
}
